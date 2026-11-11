import { lazy, Suspense, useState, useCallback, useRef, useEffect, useLayoutEffect, useMemo, type ReactNode } from "react";
import { Link, useLocation, useSearchParams } from "react-router";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { inventoryResourceNewPath } from "@/features/inventory/resourceLinks";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import {
  CheckSquare,
  Filter,
  GitBranch,
  LayoutGrid,
  List,
  MapPinned,
  Loader2,
  PlayCircle,
  Search,
  Settings,
  Terminal,
  X,
} from "lucide-react";
import {
  closeSession as apiCloseSession,
  CONNECTION_TARGETS_QUERY_KEY,
  CONNECTION_WORKSPACE_REFRESH_MS,
  listConnectionTargets,
  resumeSession,
} from "@/features/connections/api";
import type { AdHocCredentialsContext } from "@/features/connections/components/connections/AdHocCredentialsDialog";
import SessionLaunchDialog from "@/features/connections/components/connections/SessionLaunchDialog";
import { useSessionLaunch } from "@/features/connections/hooks/useSessionLaunch";
import type { ConnectionTarget, OpenSessionResponse, ViewerSessionStatus } from "@/shared/api/types";
import { Button } from "@/shared/ui-primitives/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui-primitives/tooltip";
import { Dialog, DialogContent } from "@/shared/ui-primitives/dialog";
import SearchInput from "@/shared/ui-shared/SearchInput";
import DataTable from "@/shared/ui-shared/DataTable";
import type { Column, DataTableSortState } from "@/shared/ui-shared/DataTable";
import {
  applyLocalColumnFilters,
} from "@/shared/ui-shared/dataTableLocal";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import EmptyState from "@/shared/ui-shared/EmptyState";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import { TogglePill } from "@/shared/ui-shared/TogglePill";
import { cn } from "@/shared/lib/utils";
import { useIsMobile } from "@/shared/hooks/use-mobile";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import { useSessionTouchLayout } from "@/features/connections/hooks/useSessionViewport";
import { getTauriInvoke, isPipingDesktopShell } from "@/app/desktop/runtime";
import { notify } from "@/shared/lib/notifications";
import { sshCloseMessage } from "@/features/connections/sshCloseMessage";
import InspectorPane from "./components/connections/InspectorPane";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/shared/ui-primitives/dropdown-menu";
import { connectionTargetFallbackIcon } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import ConnectionsFilters from "@/features/connections/components/connections/ConnectionsFilters";
import {
  applyFilters,
  matchesTargetSearch,
  countActiveFilters,
  deserializeFilterState,
  isSerializableFilterStateEmpty,
  normalizeFilterPresets,
  serializeFilterState,
  type SerializableFilterState,
  type FilterKey,
  type FilterPresetVisibility,
  type FilterState,
  type SavedFilterPreset,
} from "@/features/connections/components/connections/filters-utils";
import {
  createPublicConnectionFilterPreset,
  deletePublicConnectionFilterPreset,
  listPublicConnectionFilterPresets,
} from "@/features/connections/filter-presets-api";
import ConnectionInspector from "@/features/connections/components/connections/ConnectionInspector";
import ConnectionsPanel, { type DockPosition } from "@/features/connections/components/connections/ConnectionsPanel";
import { preloadLiveSessionProtocol } from "@/features/connections/components/connections/liveSessionLoaders";
import TreeView from "@/features/connections/components/connections/TreeView";
import { resolveTreeGroup, treeGroupHostId } from "@/features/connections/components/connections/treeGroups";
import CardsView from "@/features/connections/components/connections/CardsView";
import { useHostLabels } from "@/features/observability/useHostLabels";
import { HostAvailabilityIndicator } from "@/features/observability/HostAvailabilityIndicator";
import { HostObservabilityCluster } from "@/features/observability/HostObservabilityHoverCard";
import QuickAccessButtons from "@/features/connections/components/connections/QuickAccessButtons";
import { useQuickAccessIntent } from "@/features/connections/components/connections/useQuickAccessIntent";
import TreeNodeInspector, { type TreeNode } from "@/features/connections/components/connections/TreeNodeInspector";
import { CONNECTIONS_GROUP_PARAM, CONNECTIONS_HOST_PARAM, parseConnectionsGroupIdentity, serializeConnectionsGroupIdentity } from "@/features/connections/workspaceLinks";
import TreeSettingsDialog from "@/features/connections/components/connections/TreeSettingsDialog";
import {
  getPlaybookRunGroup,
  listJobTemplates,
  listProjects,
  type PlaybookRunGroup,
  type PlaybookRunGroupCreated,
  type PlaybookTargetAssignment,
  type JobTemplateVarsSchema,
  getJobTemplateVarsSchema,
} from "@/features/playbooks/api";
import PlaybookLaunchDialog, { type PlaybookLaunchIntent } from "@/features/playbooks/PlaybookLaunchDialog";
import {
  toPlaybookLaunchTarget,
  type PlaybookLaunchTargetProjection,
} from "@/features/playbooks/hostSelection";
import { launchJobOnTargets } from "@/features/playbooks/launchJobOnTargets";
import { effectiveParameters, jobVarsSchema, missingRequiredParameters, parseParameterValues, type Extravars } from "@/features/playbooks/varsSchema";
import { jobVarsSchemaQueryKey, useJobVarsSchema } from "@/features/playbooks/useJobVarsSchema";
import { launchResourceFromQuery } from "@/features/playbooks/launchReadiness";
import { AUTOMATION_SECRETS_QUERY_KEY, listAutomationSecrets } from "@/features/automation-secrets/api";
import { type RunDetailTarget } from "@/features/playbooks/RunDetailPanel";
import PlaybookRunsDock from "@/features/playbooks/PlaybookRunsDock";
import {
  buildMixedTabOrder,
  buildPlaybookTab,
  EMPTY_PERSISTED_PLAYBOOK_TABS,
  normalizePersistedPlaybookTabs,
  playbookHostSummaryFromCount,
  PLAYBOOK_TABS_SETTING_KEY,
  restorePersistedPlaybookTabs,
  toPersistedPlaybookTabs,
  type PersistedPlaybookTabs,
  type PlaybookTab,
} from "@/features/playbooks/playbookTabs";
import {
  computeAutoOrder,
  useTreeConfig,
  type TreeGroupKey,
  type TreeQuickAccessConfig,
  DEFAULT_TREE_CONFIG,
} from "@/features/connections/components/connections/treeConfig";
import {
  activePanelSessionFromLiveSession,
  clampConnectionsPanelSize,
  CONNECTIONS_PANEL_MIN_SIZES,
  CONNECTIONS_PANEL_PREFERENCE_KEY,
  defaultConnectionsPanelState,
  findPreferredPanelSession,
  normalizeConnectionsPanelState,
  type PersistedConnectionsPanelState,
  type PersistedPanelActiveSession,
} from "@/features/connections/components/connections/panelState";
import { replacePaneSession, type SessionPaneLayout } from "@/features/connections/components/connections/sessionPaneLayout";
import {
  HostStatusDot,
  HostStatusValue,
} from "@/features/connections/components/connections/HostStatus";
import { TargetBindingWarning } from "@/features/connections/components/connections/TargetBindingWarning";
import {
  hostSubtitle,
  hostValue,
} from "@/features/connections/components/connections/hostPresentation";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";
import {
  clampInspectorWidth,
  INSPECTOR_DEFAULT_WIDTH,
} from "@/features/connections/inspectorLayout";
import {
  readBrowserLocalSetting,
  readRetiredSessionJson,
  readSessionSetting,
  removeSessionSetting,
  useSetting,
  writeBrowserLocalSetting,
  writeSessionSetting,
  DEFAULT_CONNECTIONS_OPEN_IN_NEW_TAB_SETTING,
  normalizeConnectionsOpenInNewTabSetting,
  shouldOpenConnectionInNewTab,
  type ConnectionsOpenInNewTabSettingValue,
} from "@/shared/settings";
import { isSessionWsConnectable } from "@/features/connections/adapters/ws/urls";
import {
  isTerminalSessionResumeError,
  SESSION_RESUME_MAX_RETRIES,
  sessionResumeRetryDelayMs,
} from "@/features/connections/sessionRestore";
import {
  writeDetachedHandoff,
  type DetachedHandoff,
} from "@/features/connections/detachedHandoff";
import {
  buildConnectionTargetFilterOptions,
  CONNECTION_TARGETS_TABLE_CONFIG,
  connectionTargetsFilterParam,
  connectionTargetsSortParam,
  type ConnectionTargetsTableColumnKey,
} from "@/features/connections/tableConfig";

const WorkspaceMap = lazy(() => import("./WorkspaceMap"));
type ViewMode = "table" | "cards" | "tree" | "map";

const CONNECTIONS_WORKSPACE_MIN_MAIN_WIDTH_WITH_PANEL = 240;
const CONNECTIONS_WORKSPACE_MIN_MAIN_HEIGHT_WITH_PANEL = 180;
const CONNECTIONS_WORKSPACE_PANEL_GAP_PX = 12;

function scheduleDesktopVncPreload(): () => void {
  let cancelled = false;
  let frameId: number | null = null;
  let timerId: number | null = null;
  let idleId: number | null = null;
  const desktopWindow = window as Window & {
    requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number;
    cancelIdleCallback?: (handle: number) => void;
  };

  const preload = () => {
    if (cancelled) return;
    void preloadLiveSessionProtocol("vnc").catch(() => {
      // A failed warm-up should not break the workspace. The normal lazy import
      // path will surface a load error if the operator actually opens VNC.
    });
  };

  frameId = window.requestAnimationFrame(() => {
    frameId = null;
    if (desktopWindow.requestIdleCallback) {
      idleId = desktopWindow.requestIdleCallback(preload, { timeout: 1_500 });
      return;
    }
    timerId = window.setTimeout(preload, 300);
  });

  return () => {
    cancelled = true;
    if (frameId !== null) window.cancelAnimationFrame(frameId);
    if (timerId !== null) window.clearTimeout(timerId);
    if (idleId !== null) desktopWindow.cancelIdleCallback?.(idleId);
  };
}

/** Serializable shape stored in the canonical settings service (arrays instead of Sets). */
interface PersistedWorkspaceFilters {
  viewMode: ViewMode;
  filters: Partial<Record<FilterKey, string[]>>;
}

const DEFAULT_WORKSPACE_FILTERS: PersistedWorkspaceFilters = {
  viewMode: "tree",
  filters: {},
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isViewMode(value: unknown): value is ViewMode {
  return value === "table" || value === "cards" || value === "tree" || value === "map";
}

function normalizePersistedWorkspaceFilters(
  value: unknown,
  defaultViewMode: ViewMode,
): PersistedWorkspaceFilters {
  if (!isRecord(value)) {
    return { viewMode: defaultViewMode, filters: {} };
  }
  const filters: SerializableFilterState = isRecord(value.filters)
    ? serializeFilterState(deserializeFilterState(value.filters))
    : {};
  return {
    viewMode: isViewMode(value.viewMode) ? value.viewMode : defaultViewMode,
    filters,
  };
}

function toFilterState(persisted: PersistedWorkspaceFilters): FilterState {
  return deserializeFilterState(persisted.filters);
}

function toPersistedFilters(
  viewMode: ViewMode,
  filterState: FilterState,
): PersistedWorkspaceFilters {
  return { viewMode, filters: serializeFilterState(filterState) };
}

// Connection targets come from `listConnectionTargets()` via React Query; no local mock data.

const SESSION_TABS_SETTING_KEY = "connections.workspace.sessionTabs";
const RETIRED_SESSION_TABS_STORAGE_KEY = "piping.connections.workspace.sessionTabs.v1";
const PRIVATE_FILTER_PRESETS_KEY = "connections.workspace.filter-presets.private";
const PUBLIC_FILTER_PRESETS_QUERY_KEY = ["connections", "filter-presets"] as const;
const OPEN_IN_NEW_TAB_SETTING_KEY = "connections.open_in_new_tab";
const INSPECTOR_WIDTH_SETTING_KEY = "connections.workspace.inspectorWidth";

/**
 * A live connection session the workspace is displaying.
 *
 * `websocketUrl` + `token` come straight from the `POST /api/v1/connections/sessions`
 * response and are handed to the in-panel viewer (LiveTerminal for SSH,
 * guacamole-common-js adapters for RDP/VNC) so the real relay opens on mount.
 */
export interface LiveSession {
  id: string;
  hostId: string;
  hostName: string;
  // `cctv` is a client-only console tab (no proxied websocket/backend session);
  // ssh/rdp/vnc are proxied live sessions.
  protocol: "ssh" | "rdp" | "vnc" | "cctv";
  status: ViewerSessionStatus;
  websocketUrl: string;
  token: string;
  /**
   * Remote login the session authenticated as, captured from the selected
   * launch account so a detached tab can show `host · PROTOCOL · user`.
   * Absent for plain (account-less) launches.
   */
  username?: string;
  /**
   * Inventory context captured at launch so the tab tooltip can show
   * role/site/region/tenant/cluster without re-hitting the API. All optional
   * because the host may not have every field set.
   */
  hostRole?: string;
  hostSite?: string;
  hostRegion?: string;
  hostTenant?: string;
  hostCluster?: string;
}

function liveSessionFromOpenResponse(
  response: OpenSessionResponse,
  launchContext: { hostId: string; hostName: string; username?: string; host?: ConnectionTarget },
): LiveSession {
  const { session, proxy } = response;
  const host = launchContext.host;
  return {
    id: session.id,
    hostId: session.hostId,
    hostName: session.hostName ?? launchContext.hostName,
    username: launchContext.username,
    protocol: session.protocol,
    // Session is scheduled; the live viewer flips to "connected" once
    // the WS bridge opens (LiveTerminal onClose updates status on close).
    status: "connecting",
    websocketUrl: proxy.websocketUrl,
    token: proxy.token,
    hostRole: host?.role,
    hostSite: host?.site,
    hostRegion: host?.region,
    hostTenant: host?.tenant,
    hostCluster: host?.cluster,
  };
}

function isSessionProtocol(value: unknown): value is LiveSession["protocol"] {
  return value === "ssh" || value === "rdp" || value === "vnc" || value === "cctv";
}

function normalizeSessionTabs(value: unknown): LiveSession[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item): LiveSession[] => {
    if (typeof item !== "object" || item === null) return [];
    const tab = item as Partial<Pick<LiveSession, "id" | "hostId" | "hostName" | "protocol">>;
    if (
      typeof tab.id !== "string" ||
      typeof tab.hostId !== "string" ||
      typeof tab.hostName !== "string" ||
      !isSessionProtocol(tab.protocol)
    ) {
      return [];
    }
    return [{
        id: tab.id,
        hostId: tab.hostId,
        hostName: tab.hostName,
        protocol: tab.protocol,
        // A CCTV console has no websocket to resume — mark it ready on restore so
        // it renders immediately and never enters the ws resume path. Proxied
        // sessions start "connecting" and are resumed by the resume effect.
        status: tab.protocol === "cctv" ? "connected" : "connecting",
        websocketUrl: "",
        token: "",
    }];
  });
}

function readPersistedSessionTabs(): LiveSession[] {
  const current = readSessionSetting(SESSION_TABS_SETTING_KEY, [], normalizeSessionTabs);
  if (current.length > 0) return current;
  return readRetiredSessionJson(RETIRED_SESSION_TABS_STORAGE_KEY, normalizeSessionTabs) ?? [];
}

function writePersistedSessionTabs(sessions: LiveSession[]): void {
  if (sessions.length === 0) {
    removeSessionSetting(SESSION_TABS_SETTING_KEY);
    return;
  }
  writeSessionSetting(
    SESSION_TABS_SETTING_KEY,
    sessions.map((session) => ({
      id: session.id,
      hostId: session.hostId,
      hostName: session.hostName,
      protocol: session.protocol,
    })),
    normalizeSessionTabs,
  );
}

function reserveDetachedBrowserWindow(): Window | null {
  if (typeof window === "undefined" || isPipingDesktopShell({ allowStoredHint: true })) return null;
  const reserved = window.open("about:blank", "_blank");
  if (!reserved) return null;
  try {
    reserved.opener = null;
    reserved.document.title = "Piping";
  } catch {
    // Cross-origin browser policies may restrict a freshly reserved window.
  }
  return reserved;
}

function detachedWindowTitle(session: Pick<LiveSession, "hostName" | "protocol" | "username">): string {
  return [
    session.protocol.toUpperCase(),
    session.hostName,
    session.username,
  ].filter(Boolean).join(" · ");
}

function closeReservedBrowserWindow(reserved: Window | null | undefined): void {
  if (!reserved || reserved.closed) return;
  try {
    reserved.close();
  } catch {
    // Nothing useful to do if the browser refuses to close the reserved tab.
  }
}

function readPrivateFilterPresets(): SavedFilterPreset[] {
  return readBrowserLocalSetting(
    PRIVATE_FILTER_PRESETS_KEY,
    [],
    (value) => normalizeFilterPresets(value, "private"),
  );
}

function writePrivateFilterPresets(presets: SavedFilterPreset[]): void {
  writeBrowserLocalSetting(
    PRIVATE_FILTER_PRESETS_KEY,
    normalizeFilterPresets(presets, "private"),
    (value) => normalizeFilterPresets(value, "private"),
  );
}

function makeLocalPresetId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `preset-${Date.now().toString(16)}-${Math.random().toString(16).slice(2, 10)}`;
}

function workspaceRootClass(isMobile: boolean, isBottomDock: boolean): string {
  const layout = isMobile ? "flex-col h-full min-h-0" : "h-full min-h-0";
  let direction = "";
  if (isBottomDock) direction = "flex-col";
  else if (!isMobile) direction = "flex-row";
  return cn(
    "relative flex w-full max-w-full min-w-0 overflow-hidden gap-2",
    layout,
    direction,
  );
}

function mainAreaClass(
  isMobileSession: boolean,
  showPanel: boolean,
  hasOpenTabs: boolean,
  isMobile: boolean,
  isBottomDock: boolean,
): string {
  const hidden = isMobileSession && showPanel && hasOpenTabs;
  let sizing: string;
  if (isMobile) sizing = "flex-col min-h-0";
  else if (isBottomDock) sizing = "min-h-[180px]";
  else sizing = "min-w-[240px] min-h-0";
  return cn(
    "flex min-w-0 max-w-full gap-4 flex-1 overflow-hidden",
    hidden && "hidden",
    sizing,
  );
}

function filterButtonClass(activeFilterCount: number, showFilters: boolean): string {
  let state: string;
  if (activeFilterCount > 0) state = "bg-accent text-primary border border-primary/45";
  else if (showFilters) state = "bg-accent text-primary border border-primary/40";
  else state = "text-muted-foreground hover:text-foreground hover:bg-accent";
  return cn(
    "relative h-9 w-9 shrink-0 rounded-md flex items-center justify-center surface-chip transition-colors",
    state,
  );
}

function ViewModeIcon({ mode }: Readonly<{ mode: ViewMode }>) {
  if (mode === "tree") return <GitBranch className="size-4" />;
  if (mode === "cards") return <LayoutGrid className="size-4" />;
  if (mode === "map") return <MapPinned className="size-4" />;
  return <List className="size-4" />;
}

type WorkspaceLaunchDestination =
  | { kind: "new"; host: ConnectionTarget; openInNewTab: boolean; reservedWindow: Window | null }
  | { kind: "replace"; previous: LiveSession };

const ConnectionsWorkspace = ({ visible = true }: { visible?: boolean }) => {
  const { t } = useTranslation();
  const isMobile = useIsMobile();
  const queryClient = useQueryClient();
  const { intentStartedAt: tableIntentStartedAt, markIntent: markTableIntent } = useQuickAccessIntent();
  const [catalogueRetrying, setCatalogueRetrying] = useState(false);
  useEffect(() => {
    if (!isPipingDesktopShell({ allowStoredHint: true })) return;
    return scheduleDesktopVncPreload();
  }, []);
  // Persisted workspace state (viewMode + filters) — debounced 500ms so rapid filter
  // changes don't flood the API with requests.
  const defaultWorkspaceFilters = useMemo<PersistedWorkspaceFilters>(
    () => ({ ...DEFAULT_WORKSPACE_FILTERS, viewMode: isMobile ? "cards" : "tree" }),
    [isMobile],
  );
  const {
    value: persistedWorkspaceRaw,
    setValue: setPersistedWorkspaceRaw,
    isLoading: persistedLoading,
  } = useSetting<unknown>(
    "connections.workspace.filters",
    defaultWorkspaceFilters,
    { debounceMs: 500 },
  );
  const persistedWorkspace = useMemo(
    () => normalizePersistedWorkspaceFilters(
      persistedWorkspaceRaw,
      defaultWorkspaceFilters.viewMode,
    ),
    [defaultWorkspaceFilters.viewMode, persistedWorkspaceRaw],
  );
  // Workspace layout and filter preferences are debounced UI state: `setValue`
  // applies optimistically, never rejects, and rolls the stored value back when
  // the write fails, so its settle result is deliberately not reported here.
  const setPersistedWorkspace = useCallback(
    (
      next:
        | PersistedWorkspaceFilters
        | ((prev: PersistedWorkspaceFilters) => PersistedWorkspaceFilters),
    ) => {
      void setPersistedWorkspaceRaw((prevRaw: unknown) => {
        const prev = normalizePersistedWorkspaceFilters(
          prevRaw,
          defaultWorkspaceFilters.viewMode,
        );
        return typeof next === "function" ? next(prev) : next;
      });
    },
    [defaultWorkspaceFilters.viewMode, setPersistedWorkspaceRaw],
  );

  const [viewMode, setViewModeState] = useState<ViewMode>(() =>
    persistedWorkspace.viewMode,
  );
  const [filterState, setFilterStateRaw] = useState<FilterState>(() =>
    toFilterState(persistedWorkspace),
  );
  const [tableSortState, setTableSortState] = useState<DataTableSortState | null>(null);
  const [tableColumnFilters, setTableColumnFilters] = useState<Record<string, string[]>>({});
  const [tablePage, setTablePage] = useState(0);
  const { value: tablePageSize, setValue: setTablePageSize } = useTablePageSize();
  const viewModeRef = useRef(viewMode);
  const filterStateRef = useRef(filterState);
  const workspaceStateTouchedRef = useRef(false);
  useEffect(() => {
    viewModeRef.current = viewMode;
  }, [viewMode]);
  useEffect(() => {
    filterStateRef.current = filterState;
  }, [filterState]);
  const activeFilterCount = countActiveFilters(filterState);
  const [privateFilterPresetValue, setPrivateFilterPresetValueState] = useState<SavedFilterPreset[]>(
    readPrivateFilterPresets,
  );
  const setPrivateFilterPresetValue = useCallback(
    (
      next:
        | SavedFilterPreset[]
        | ((prev: SavedFilterPreset[]) => SavedFilterPreset[]),
    ) => {
      setPrivateFilterPresetValueState((prev) => {
        const resolved = typeof next === "function" ? next(prev) : next;
        const normalized = normalizeFilterPresets(resolved, "private");
        writePrivateFilterPresets(normalized);
        return normalized;
      });
    },
    [],
  );
  const privateFilterPresetsLoading = false;
  const privateFilterPresets = useMemo(
    () => normalizeFilterPresets(privateFilterPresetValue, "private"),
    [privateFilterPresetValue],
  );
  const publicFilterPresetsQuery = useQuery({
    queryKey: PUBLIC_FILTER_PRESETS_QUERY_KEY,
    queryFn: () => listPublicConnectionFilterPresets(),
  });
  const publicFilterPresets = publicFilterPresetsQuery.data ?? [];
  const isMobileSession = useSessionTouchLayout();
  const currentTableColumnKeySet = useMemo(
    () => new Set<string>(CONNECTION_TARGETS_TABLE_CONFIG.viewColumns),
    [],
  );

  const createPublicPresetMutation = useMutation({
    mutationFn: createPublicConnectionFilterPreset,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: PUBLIC_FILTER_PRESETS_QUERY_KEY });
    },
  });
  const deletePublicPresetMutation = useMutation({
    mutationFn: deletePublicConnectionFilterPreset,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: PUBLIC_FILTER_PRESETS_QUERY_KEY });
    },
  });
  const deletingPublicPresetId = deletePublicPresetMutation.isPending
    ? (deletePublicPresetMutation.variables ?? null)
    : null;

  // Sync persisted value into local state once it loads from API.
  const persistedLoadedRef = useRef(false);
  useEffect(() => {
    if (persistedLoadedRef.current || persistedLoading) return;
    persistedLoadedRef.current = true;
    if (workspaceStateTouchedRef.current) return;
    const nextFilterState = toFilterState(persistedWorkspace);
    viewModeRef.current = persistedWorkspace.viewMode;
    filterStateRef.current = nextFilterState;
    setViewModeState(persistedWorkspace.viewMode);
    setFilterStateRaw(nextFilterState);
  }, [persistedLoading, persistedWorkspace]);

  const setViewMode = (mode: ViewMode) => {
    workspaceStateTouchedRef.current = true;
    viewModeRef.current = mode;
    setViewModeState(mode);
    setPersistedWorkspace(toPersistedFilters(mode, filterStateRef.current));
  };

  const setFilterState = (next: FilterState) => {
    setTablePage(0);
    workspaceStateTouchedRef.current = true;
    filterStateRef.current = next;
    setFilterStateRaw(next);
    setPersistedWorkspace(toPersistedFilters(viewModeRef.current, next));
  };

  const saveFilterPreset = useCallback(
    async (name: string, visibility: FilterPresetVisibility, draft: FilterState) => {
      const filters = serializeFilterState(draft);
      if (isSerializableFilterStateEmpty(filters)) {
        notify("error", t("connections.filtersPanel.emptyPreset"));
        return;
      }
      const trimmedName = name.trim();
      if (!trimmedName) {
        notify("error", t("connections.filtersPanel.filterNameRequired"));
        return;
      }

      if (visibility === "public") {
        try {
          await createPublicPresetMutation.mutateAsync({ name: trimmedName, filters });
          notify("success", t("connections.filtersPanel.presetSaved"));
        } catch (error) {
          notify("error", t("connections.filtersPanel.presetSaveFailed"));
          throw error;
        }
        return;
      }

      const now = new Date().toISOString();
      const preset: SavedFilterPreset = {
        id: makeLocalPresetId(),
        name: trimmedName,
        visibility: "private",
        filters,
        createdAt: now,
        updatedAt: now,
      };
      setPrivateFilterPresetValue((prev) => [preset, ...normalizeFilterPresets(prev, "private")]);
      notify("success", t("connections.filtersPanel.presetSaved"));
    },
    [createPublicPresetMutation, setPrivateFilterPresetValue, t],
  );

  const applyFilterPreset = useCallback(
    (preset: SavedFilterPreset) => {
      setTablePage(0);
      const next = deserializeFilterState(preset.filters);
      workspaceStateTouchedRef.current = true;
      filterStateRef.current = next;
      setFilterStateRaw(next);
      setPersistedWorkspace(toPersistedFilters(viewModeRef.current, next));
      notify("success", t("connections.filtersPanel.presetApplied"));
    },
    [setPersistedWorkspace, t],
  );

  const deleteFilterPreset = useCallback(
    async (preset: SavedFilterPreset) => {
      if (preset.visibility === "private") {
        setPrivateFilterPresetValue((prev) =>
          normalizeFilterPresets(prev, "private").filter((candidate) => candidate.id !== preset.id),
        );
        notify("success", t("connections.filtersPanel.presetDeleted"));
        return;
      }

      try {
        await deletePublicPresetMutation.mutateAsync(preset.id);
        notify("success", t("connections.filtersPanel.presetDeleted"));
      } catch (error) {
        notify("error", t("connections.filtersPanel.presetDeleteFailed"), {
          description: error instanceof Error ? error.message : "",
        });
      }
    },
    [deletePublicPresetMutation, setPrivateFilterPresetValue, t],
  );

  const [searchQuery, setSearchQuery] = useState("");
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const filtersButtonRef = useRef<HTMLButtonElement | null>(null);
  // Debounced copy of the query: filtering runs over the full host list (up to
  // thousands of rows), so we recompute on a 200ms pause rather than every
  // keystroke to keep typing responsive.
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const normalizedSearch = debouncedSearch.trim();
  useEffect(() => {
    const id = setTimeout(() => setDebouncedSearch(searchQuery), 200);
    return () => clearTimeout(id);
  }, [searchQuery]);
  const clearSearch = useCallback(() => {
    setTablePage(0);
    setSearchQuery("");
    setDebouncedSearch("");
    window.requestAnimationFrame(() => searchInputRef.current?.focus());
  }, []);
  const [selectedHost, setSelectedHost] = useState<ConnectionTarget | null>(null);
  const [selectedNode, setSelectedNode] = useState<TreeNode | null>(null);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [searchParams, setSearchParams] = useSearchParams();
  const revealNavigationKey = useLocation().key;
  const deepLinkAppliedRef = useRef<string | null>(null);
  const workspaceVisibleRef = useRef(visible);
  const [previousVisibility, setPreviousVisibility] = useState(visible);
  if (previousVisibility !== visible) {
    setPreviousVisibility(visible);
    if (!visible) {
      setInspectorOpen(false);
      setSelectedNode(null);
      setSelectedHost(null);
    }
  }
  useLayoutEffect(() => {
    workspaceVisibleRef.current = visible;
    if (!visible) deepLinkAppliedRef.current = null;
  }, [visible]);
  const dismissInspector = useCallback(() => {
    setInspectorOpen(false);
    deepLinkAppliedRef.current = null;
    if (!workspaceVisibleRef.current) return;
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete(CONNECTIONS_HOST_PARAM);
      next.delete(CONNECTIONS_GROUP_PARAM);
      return next;
    }, { replace: true });
  }, [setSearchParams]);
  const selectUrlNode = useCallback((node: TreeNode) => {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete(CONNECTIONS_HOST_PARAM);
      next.delete(CONNECTIONS_GROUP_PARAM);
      if (node.type === "host") next.set(CONNECTIONS_HOST_PARAM, node.id);
      else {
        const anchor = node.hosts?.find((host) => treeGroupHostId(host, node.type as TreeGroupKey)) ?? node.hosts?.[0];
        if (anchor) next.set(CONNECTIONS_GROUP_PARAM, serializeConnectionsGroupIdentity({
          type: node.type, path: node.id, anchor: anchor.id,
          entityId: treeGroupHostId(anchor, node.type) ?? null,
        }));
      }
      return next;
    }, { replace: true });
  }, [setSearchParams]);
  const [sessionFocusRequest, setSessionFocusRequest] = useState(0);
  const inspectorOpenerRef = useRef<{ element: HTMLElement | null; session: boolean }>({ element: null, session: false });
  const inspectorFocusFrameRef = useRef<number | null>(null);
  useEffect(() => () => {
    if (inspectorFocusFrameRef.current !== null) window.cancelAnimationFrame(inspectorFocusFrameRef.current);
  }, []);
  const rememberInspectorOpener = useCallback((session = false) => {
    if (inspectorFocusFrameRef.current !== null) window.cancelAnimationFrame(inspectorFocusFrameRef.current);
    const element = document.activeElement;
    inspectorOpenerRef.current = {
      element: element instanceof HTMLElement && element !== document.body ? element : null,
      session,
    };
    setInspectorOpen(true);
  }, []);
  const inspectHost = useCallback((host: ConnectionTarget, fromSession = false) => {
    setSelectedHost(host);
    setSelectedNode({ type: "host", id: host.id, name: host.name, host, hosts: [host] });
    selectUrlNode({ type: "host", id: host.id, name: host.name });
    rememberInspectorOpener(fromSession);
  }, [rememberInspectorOpener, selectUrlNode]);
  const inspectNode = useCallback((node: TreeNode) => {
    setSelectedNode(node);
    setSelectedHost(node.type === "host" ? node.host ?? null : null);
    selectUrlNode(node);
    rememberInspectorOpener();
  }, [rememberInspectorOpener, selectUrlNode]);
  const focusSession = useCallback(() => {
    if (inspectorFocusFrameRef.current !== null) window.cancelAnimationFrame(inspectorFocusFrameRef.current);
    dismissInspector();
    setSessionFocusRequest(request => request + 1);
  }, [dismissInspector]);
  const closeInspector = useCallback(() => {
    if (inspectorFocusFrameRef.current !== null) window.cancelAnimationFrame(inspectorFocusFrameRef.current);
    dismissInspector();
    const opener = inspectorOpenerRef.current;
    if (opener.session) {
      focusSession();
    }
    // Touch sessions deliberately do not focus the terminal and open the OS keyboard.
    if (!opener.session || isMobileSession) {
      inspectorFocusFrameRef.current = window.requestAnimationFrame(() => {
        inspectorFocusFrameRef.current = null;
        if (opener.element?.isConnected) opener.element.focus();
        else searchInputRef.current?.focus();
      });
    }
  }, [dismissInspector, focusSession, isMobileSession]);
  // Ad-hoc credentials dialog context (mounted once at the workspace level so
  // its portal never competes with the session panel).
  const [bulkSelectionMode, setBulkSelectionMode] = useState(false);
  const [bulkSelectedHostIds, setBulkSelectedHostIds] = useState<Set<string>>(() => new Set());
  const [bulkLaunchDialogOpen, setBulkLaunchDialogOpen] = useState(false);
  const [pendingHostLaunch, setPendingHostLaunch] = useState<{ host: ConnectionTarget; item: PlaybookTargetAssignment; varsSchema?: JobTemplateVarsSchema } | null>(null);
  const [bulkLaunchJobId, setBulkLaunchJobId] = useState("");
  const [showFilters, setShowFilters] = useState(false);
  const [sessions, setSessions] = useState<LiveSession[]>(() => readPersistedSessionTabs());
  // Playbook run tabs — a SEPARATE array from `sessions[]`, merged
  // only at the render/Alt+N layer. Persisted server-side (separate settings
  // key from `connections.workspace.sessionTabs`) so a live run survives a
  // reload; restored tabs are dropped once their run reaches a terminal status.
  const [playbookTabs, setPlaybookTabs] = useState<PlaybookTab[]>([]);
  const [activePlaybookTabId, setActivePlaybookTabId] = useState<string | null>(null);
  const {
    value: persistedPlaybookTabs,
    setValue: setPersistedPlaybookTabs,
    isLoading: persistedPlaybookTabsLoading,
  } = useSetting<PersistedPlaybookTabs>(
    PLAYBOOK_TABS_SETTING_KEY,
    EMPTY_PERSISTED_PLAYBOOK_TABS,
    { debounceMs: 300, normalize: normalizePersistedPlaybookTabs },
  );
  const playbookTabsRestoredRef = useRef(false);
  const playbookTabsRestoreInflightRef = useRef(false);
  const sessionResumeInflightRef = useRef<Set<string>>(new Set());
  const sessionResumeAttemptsRef = useRef<Map<string, number>>(new Map());
  const sessionResumeTimersRef = useRef<Map<string, number>>(new Map());
  const resumePersistedSessionRef = useRef<(sessionToResume: LiveSession) => void>(() => undefined);
  const defaultPanelState = useMemo(
    () => defaultConnectionsPanelState(isMobile),
    [isMobile],
  );
  const normalizePanelPreference = useCallback(
    (value: unknown) => normalizeConnectionsPanelState(value, isMobile),
    [isMobile],
  );
  const {
    value: persistedPanelState,
    setValue: setPersistedPanelState,
    isLoading: panelPreferenceLoading,
  } = useSetting<PersistedConnectionsPanelState>(
    CONNECTIONS_PANEL_PREFERENCE_KEY,
    defaultPanelState,
    {
      debounceMs: 300,
      normalize: normalizePanelPreference,
    },
  );
  const initialPanelState = normalizeConnectionsPanelState(defaultPanelState, isMobile);
  const [showPanel, setShowPanel] = useState(initialPanelState.showPanel);
  const [activeSessionId, setActiveSessionId] = useState<string | null>(
    initialPanelState.activeSession?.sessionId ?? null,
  );
  const [activePanelSession, setActivePanelSession] = useState<PersistedPanelActiveSession | null>(
    initialPanelState.activeSession,
  );
  const [panelExpanded, setPanelExpanded] = useState(initialPanelState.expanded);
  const [dockPosition, setDockPosition] = useState<DockPosition>(initialPanelState.dockPosition);
  const [panelSizes, setPanelSizes] = useState<Record<DockPosition, number>>(
    initialPanelState.sizes,
  );
  const panelStateTouchedRef = useRef(false);
  const [treeSettingsOpen, setTreeSettingsOpen] = useState(false);
  const { config: treeConfigData, setConfig: setTreeConfig, isLoading: treeConfigLoading } = useTreeConfig();
  const { value: openInNewTabSetting } = useSetting<ConnectionsOpenInNewTabSettingValue>(
    OPEN_IN_NEW_TAB_SETTING_KEY,
    DEFAULT_CONNECTIONS_OPEN_IN_NEW_TAB_SETTING,
    { normalize: normalizeConnectionsOpenInNewTabSetting },
  );

  const updatePersistedPanelState = useCallback(
    (patch: Partial<PersistedConnectionsPanelState>) => {
      panelStateTouchedRef.current = true;
      void setPersistedPanelState((prev) => {
        const current = normalizeConnectionsPanelState(prev, isMobile);
        return normalizeConnectionsPanelState({ ...current, ...patch }, isMobile);
      });
    },
    [isMobile, setPersistedPanelState],
  );

  const rememberPanelVisibility = useCallback(
    (visible: boolean) => {
      setShowPanel(visible);
      updatePersistedPanelState({ showPanel: visible });
    },
    [setShowPanel, updatePersistedPanelState],
  );

  const rememberPanelExpanded = useCallback(
    (expanded: boolean) => {
      setPanelExpanded(expanded);
      updatePersistedPanelState({ expanded });
    },
    [setPanelExpanded, updatePersistedPanelState],
  );

  const rememberPanelOpen = useCallback(
    (expanded = true) => {
      focusSession();
      setShowPanel(true);
      setPanelExpanded(expanded);
      updatePersistedPanelState({ showPanel: true, expanded });
    },
    [focusSession, setPanelExpanded, setShowPanel, updatePersistedPanelState],
  );

  const rememberDockPosition = useCallback(
    (nextDockPosition: DockPosition) => {
      focusSession();
      setDockPosition(nextDockPosition);
      updatePersistedPanelState({ dockPosition: nextDockPosition });
    },
    [focusSession, setDockPosition, updatePersistedPanelState],
  );

  const rememberPanelSize = useCallback(
    (size: number, sizeDockPosition: DockPosition, availableSize?: number) => {
      setPanelSizes((currentSizes) => {
        const sizes = {
          ...currentSizes,
          [sizeDockPosition]: clampConnectionsPanelSize(sizeDockPosition, size, availableSize),
        };
        updatePersistedPanelState({ sizes });
        return sizes;
      });
    },
    [setPanelSizes, updatePersistedPanelState],
  );

  const rememberActivePanelSession = useCallback(
    (session: LiveSession | null) => {
      const activeSession = session ? activePanelSessionFromLiveSession(session) : null;
      setActiveSessionId(session?.id ?? null);
      setActivePanelSession(activeSession);
      updatePersistedPanelState({ activeSession });
      // Activating a real session always wins over a playbook tab (mutual
      // exclusion with activePlaybookTabId — see selectPlaybookTab/openPlaybookTab
      // for the reverse direction). `session === null` is a pure clear (e.g. the
      // last session closed) and must NOT touch the playbook selection — a
      // remaining playbook tab should stay active.
      if (session) setActivePlaybookTabId(null);
    },
    [setActivePanelSession, setActiveSessionId, updatePersistedPanelState],
  );

  const activatePanelSession = useCallback(
    (session: LiveSession, options?: { showPanel?: boolean; expanded?: boolean }) => {
      focusSession();
      const activeSession = activePanelSessionFromLiveSession(session);
      setActiveSessionId(session.id);
      setActivePanelSession(activeSession);
      setActivePlaybookTabId(null);
      if (options?.showPanel === true) setShowPanel(true);
      if (options?.expanded !== undefined) setPanelExpanded(options.expanded);
      updatePersistedPanelState({
        activeSession,
        ...(options?.showPanel === true ? { showPanel: true } : {}),
        ...(options?.expanded !== undefined ? { expanded: options.expanded } : {}),
      });
    },
    [
      focusSession,
      setActivePanelSession,
      setActiveSessionId,
      setPanelExpanded,
      setShowPanel,
      updatePersistedPanelState,
    ],
  );

  const attachPanelSession = useCallback(
    (session: LiveSession) => {
      setSessions((prev) => [
        ...prev.filter((candidate) => candidate.id !== session.id),
        session,
      ]);
      activatePanelSession(session, { showPanel: true, expanded: true });
    },
    [activatePanelSession, setSessions],
  );

  /**
   * Called whenever the LAST session goes away (closed/detached, single or
   * all). Only hides the merged panel when there is truly nothing left to
   * show — an open playbook run tab must keep the panel visible. If nothing
   * is active on the playbook side either, surface the last one instead of
   * leaving the panel blank.
   */
  const hidePanelOrActivateRemainingPlaybookTab = useCallback(() => {
    if (playbookTabs.length === 0) {
      rememberPanelVisibility(false);
      return;
    }
    if (activePlaybookTabId === null) {
      const lastPlaybookTab = playbookTabs[playbookTabs.length - 1];
      if (lastPlaybookTab) setActivePlaybookTabId(lastPlaybookTab.id);
    }
  }, [activePlaybookTabId, playbookTabs, rememberPanelVisibility]);

  const removeLocalSession = useCallback(
    (sessionId: string) => {
      const remaining = sessions.filter((s) => s.id !== sessionId);
      if (remaining.length === sessions.length) return;

      setSessions(remaining);
      if (activeSessionId === sessionId) {
        const last = remaining[remaining.length - 1];
        rememberActivePanelSession(last ?? null);
        if (remaining.length === 0) hidePanelOrActivateRemainingPlaybookTab();
      }
    },
    [
      activeSessionId,
      hidePanelOrActivateRemainingPlaybookTab,
      rememberActivePanelSession,
      sessions,
      setSessions,
    ],
  );

  const openDetachedSession = useCallback(
    (
      session: LiveSession,
      reservedWindow?: Window | null,
      onFailed?: () => void,
      onOpened?: () => void,
    ): boolean => {
      const detachedPath = `/connections/detached/${encodeURIComponent(session.id)}`;
      const handoff: DetachedHandoff = {
        sessionId: session.id,
        hostName: session.hostName,
        hostId: session.hostId,
        protocol: session.protocol,
        websocketUrl: session.websocketUrl,
        token: session.token,
        ...(session.username ? { username: session.username } : {}),
      };
      writeDetachedHandoff(session.id, handoff);
      const title = detachedWindowTitle(session);

      const desktopInvoke = getTauriInvoke();
      if (isPipingDesktopShell()) {
        closeReservedBrowserWindow(reservedWindow);
        if (!desktopInvoke) {
          notify("error", t("connections.detachSession"), {
            description: t("connections.appLaunch.desktopOpenFailed"),
            source: "connections",
          });
          onFailed?.();
          return false;
        }
        void desktopInvoke("open_detached_window", { path: detachedPath, handoff, title })
          .then(() => onOpened?.())
          .catch((error) => {
            const description = error instanceof Error ? error.message : String(error);
            notify("error", t("connections.detachSession"), {
              description,
              source: "connections",
            });
            onFailed?.();
          });
        return true;
      }

      if (reservedWindow && !reservedWindow.closed) {
        try {
          reservedWindow.document.title = title;
          reservedWindow.location.href = detachedPath;
          onOpened?.();
          return true;
        } catch {
          closeReservedBrowserWindow(reservedWindow);
        }
      }

      const detachedWindow = window.open(detachedPath, "_blank");
      if (!detachedWindow && !isPipingDesktopShell()) {
        notify("warning", t("connections.detachSession"), {
          description: t("connections.popupBlocked"),
          source: "connections",
        });
        onFailed?.();
        return false;
      }
      onOpened?.();
      return true;
    },
    [t],
  );

  const selectPanelSession = useCallback(
    (sessionId: string | null) => {
      const session = sessions.find((candidate) => candidate.id === sessionId);
      if (session) {
        activatePanelSession(session);
        return;
      }
      setActiveSessionId(sessionId);
      setActivePanelSession(sessionId ? { sessionId } : null);
      setActivePlaybookTabId(null);
      updatePersistedPanelState({ activeSession: sessionId ? { sessionId } : null });
    },
    [activatePanelSession, sessions, updatePersistedPanelState],
  );

  const selectPlaybookTab = useCallback(
    (id: string) => {
      dismissInspector();
      setActivePlaybookTabId(id);
      // Full clear (id + activePanelSession + persisted activeSession) — a bare
      // `setActiveSessionId(null)` left `activePanelSession` populated, which fed
      // the preferred-session restore effect and re-activated a session on the
      // very next render (both viewers would then render at once).
      rememberActivePanelSession(null);
    },
    [dismissInspector, rememberActivePanelSession],
  );

  const closePlaybookTab = useCallback(
    (id: string) => {
      const remaining = playbookTabs.filter((tab) => tab.id !== id);
      if (remaining.length === playbookTabs.length) return;
      setPlaybookTabs(remaining);
      if (activePlaybookTabId === id) {
        const last = remaining[remaining.length - 1];
        setActivePlaybookTabId(last?.id ?? null);
        if (remaining.length === 0 && sessions.length === 0) rememberPanelVisibility(false);
      }
    },
    [activePlaybookTabId, playbookTabs, rememberPanelVisibility, sessions.length],
  );

  /** Opens a NEW playbook run as a tab — replaces the old `runDetail` column. */
  const openPlaybookTab = useCallback(
    (target: RunDetailTarget, playbookName: string) => {
      const tab = buildPlaybookTab(target, playbookName, t);
      setPlaybookTabs((prev) => [...prev.filter((candidate) => candidate.id !== tab.id), tab]);
      setActivePlaybookTabId(tab.id);
      rememberActivePanelSession(null);
      rememberPanelOpen(true);
    },
    [rememberActivePanelSession, rememberPanelOpen, t],
  );

  /** Dock row click: focus the tab if it's already open, else open it. */
  const openOrFocusPlaybookTab = useCallback(
    (group: PlaybookRunGroup, playbookName: string) => {
      const existing = playbookTabs.find((tab) => tab.groupId === group.id);
      if (existing) {
        setActivePlaybookTabId(existing.id);
        rememberActivePanelSession(null);
        rememberPanelOpen(true);
        return;
      }
      const tab: PlaybookTab = {
        id: group.id,
        groupId: group.id,
        playbookName,
        hostSummary: playbookHostSummaryFromCount(group.total_runs, t),
        phase: group.status,
        target: { kind: "group", groupId: group.id },
        startedAt: group.started_at ?? group.created_at,
      };
      setPlaybookTabs((prev) => [...prev.filter((candidate) => candidate.id !== tab.id), tab]);
      setActivePlaybookTabId(tab.id);
      rememberActivePanelSession(null);
      rememberPanelOpen(true);
    },
    [playbookTabs, rememberActivePanelSession, rememberPanelOpen, t],
  );

  // Alt+1..9 jumps to the n-th open tab (1-indexed): session tabs first, then
  // playbook run tabs. Mirrors the IDE/browser convention. Skipped when the
  // panel is not visible, when no tab sits at that slot, or while the user is
  // typing in a form/contenteditable field. The terminal/RDP/VNC viewers run
  // inside iframes/canvases, so this handler does not steal keys from a
  // focused remote session.
  useEffect(() => {
    if (!showPanel) return;
    const order = buildMixedTabOrder(
      sessions.map((s) => s.id),
      playbookTabs.map((t) => t.id),
    );
    if (order.length === 0) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
      if (event.key < "1" || event.key > "9") return;
      const target = event.target as HTMLElement | null;
      const tag = target?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if (target?.isContentEditable) return;
      const index = Number(event.key) - 1;
      if (index >= order.length) return;
      const ref = order[index];
      if (!ref) return;
      event.preventDefault();
      if (ref.kind === "session") selectPanelSession(ref.id);
      else selectPlaybookTab(ref.id);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectPanelSession, selectPlaybookTab, sessions, playbookTabs, showPanel]);

  // Derive granular setters from setConfig to pass to TreeSettingsDialog
  const treeConfig = {
    config: treeConfigData,
    setMode: (mode: "auto" | "custom", seedOrder?: TreeGroupKey[]) => {
      if (mode === "custom" && seedOrder && seedOrder.length > 0) {
        setTreeConfig({ ...treeConfigData, mode, order: seedOrder });
      } else {
        setTreeConfig({ ...treeConfigData, mode });
      }
    },
    addGroup: (key: TreeGroupKey) => {
      if (!treeConfigData.order.includes(key)) {
        setTreeConfig({ ...treeConfigData, order: [...treeConfigData.order, key] });
      }
    },
    removeGroup: (key: TreeGroupKey) => {
      setTreeConfig({ ...treeConfigData, order: treeConfigData.order.filter((k) => k !== key) });
    },
    moveGroup: (key: TreeGroupKey, dir: "up" | "down") => {
      const idx = treeConfigData.order.indexOf(key);
      if (idx < 0) return;
      const to = dir === "up" ? idx - 1 : idx + 1;
      if (to < 0 || to >= treeConfigData.order.length) return;
      const next = [...treeConfigData.order];
      const a = next[idx];
      const b = next[to];
      if (a === undefined || b === undefined) return;
      next[idx] = b;
      next[to] = a;
      setTreeConfig({ ...treeConfigData, order: next });
    },
    setIncludeUnknown: (v: boolean) => {
      setTreeConfig({ ...treeConfigData, includeUnknown: v });
    },
    setQuickAccess: (quickAccess: TreeQuickAccessConfig) => {
      setTreeConfig({ ...treeConfigData, quickAccess });
    },
    reset: () => {
      setTreeConfig(DEFAULT_TREE_CONFIG);
    },
  };

  const panelPreferenceLoadedRef = useRef(false);
  useEffect(() => {
    if (panelPreferenceLoadedRef.current || panelPreferenceLoading) return;
    panelPreferenceLoadedRef.current = true;
    if (panelStateTouchedRef.current) return;
    const next = normalizeConnectionsPanelState(persistedPanelState, isMobile);

    // External preference hydration: this UI state is intentionally user-controlled.
    setShowPanel(next.showPanel);
    setPanelExpanded(next.expanded);
    setDockPosition(next.dockPosition);
    setPanelSizes(next.sizes);
    setActivePanelSession(next.activeSession);
    setActiveSessionId(next.activeSession?.sessionId ?? null);
  }, [isMobile, panelPreferenceLoading, persistedPanelState]);

  const hostsQuery = useQuery({
    queryKey: CONNECTION_TARGETS_QUERY_KEY,
    queryFn: () => listConnectionTargets(),
    // Silent background refresh: structural sharing + stable tree state mean an
    // unchanged poll re-renders nothing and a changed row updates in place.
    refetchInterval: CONNECTION_WORKSPACE_REFRESH_MS,
    refetchIntervalInBackground: false,
    refetchOnWindowFocus: true,
  });
  // `/api/v1/connections/targets` returns the hydrated connection-target workspace contract;
  // components must render only fields that are actually present in that row.
  const hosts: ConnectionTarget[] = useMemo(() => hostsQuery.data?.items ?? [], [hostsQuery.data]);
  const sanitizedTableSortState = useMemo(
    () => sanitizeDataTableSortState(tableSortState, currentTableColumnKeySet),
    [currentTableColumnKeySet, tableSortState],
  );
  const sanitizedTableColumnFilters = useMemo(
    () => sanitizeDataTableColumnFilters(tableColumnFilters, currentTableColumnKeySet),
    [currentTableColumnKeySet, tableColumnFilters],
  );
  const tableServerSort = sanitizedTableSortState
    ? connectionTargetsSortParam(sanitizedTableSortState.key)
    : undefined;
  const tableServerFilters = useMemo(() => {
    const filters: Record<string, string[]> = {};
    for (const [columnKey, values] of Object.entries(sanitizedTableColumnFilters)) {
      if (values.length === 0) continue;
      const backendParam = connectionTargetsFilterParam(columnKey);
      if (backendParam) filters[backendParam] = values;
    }
    return filters;
  }, [sanitizedTableColumnFilters]);
  const localTableColumnFilters = useMemo(() => {
    const filters: Record<string, string[]> = {};
    for (const [columnKey, values] of Object.entries(sanitizedTableColumnFilters)) {
      if (values.length === 0) continue;
      if (connectionTargetsFilterParam(columnKey)) continue;
      filters[columnKey] = values;
    }
    return filters;
  }, [sanitizedTableColumnFilters]);
  const tableServerQueryEnabled =
    viewMode === "table" &&
    (
      Boolean(tableServerSort) ||
      Object.keys(tableServerFilters).length > 0 ||
      normalizedSearch.length > 0
    );
  const tableHostsQuery = useQuery({
    queryKey: [
      ...CONNECTION_TARGETS_QUERY_KEY,
      "table",
      {
        search: normalizedSearch,
        sort: tableServerSort,
        order: sanitizedTableSortState?.dir,
        filters: tableServerFilters,
      },
    ],
    queryFn: () =>
      listConnectionTargets({
        search: normalizedSearch,
        sort: tableServerSort,
        order: sanitizedTableSortState?.dir,
        filters: tableServerFilters,
      }),
    enabled: tableServerQueryEnabled,
    // Same silent background refresh in server-side table mode; placeholderData
    // keeps the current rows visible across both param changes and refetches.
    placeholderData: (prev) => prev,
    refetchInterval: CONNECTION_WORKSPACE_REFRESH_MS,
    refetchIntervalInBackground: false,
    refetchOnWindowFocus: true,
  });
  const tableHosts = useMemo(
    () => (tableServerQueryEnabled ? tableHostsQuery.data?.items ?? [] : hosts),
    [hosts, tableHostsQuery.data, tableServerQueryEnabled],
  );

  // Measure the workspace, not a column that changes width when details open.
  const workspaceRef = useRef<HTMLDivElement>(null);
  const [workspaceBounds, setWorkspaceBounds] = useState({ width: 1200, height: 600 });
  useEffect(() => {
    if (!workspaceRef.current) return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        setWorkspaceBounds({ width: e.contentRect.width, height: e.contentRect.height });
      }
    });
    ro.observe(workspaceRef.current);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    writePersistedSessionTabs(sessions);
  }, [sessions]);

  // Restore persisted playbook tabs on boot: fetch each group's live status in
  // parallel and keep only the non-terminal ones (constraint #4 — a run that
  // finishes while its tab is open stays open until the user closes it, but a
  // reload never resurrects an already-finished run as a tab).
  useEffect(() => {
    if (playbookTabsRestoredRef.current || playbookTabsRestoreInflightRef.current) return;
    if (persistedPlaybookTabsLoading) return;
    if (persistedPlaybookTabs.tabs.length === 0) {
      playbookTabsRestoredRef.current = true;
      return;
    }
    playbookTabsRestoreInflightRef.current = true;
    void restorePersistedPlaybookTabs(persistedPlaybookTabs, (groupId) => getPlaybookRunGroup(groupId))
      .then((restored) => {
        // Merge, don't replace: the workspace is fully interactive during this
        // network round-trip, so the user may have already opened a NEW
        // playbook tab (launch dialog, dock row) for a group that wasn't in
        // the persisted set. A plain `setPlaybookTabs(restored)` would wipe
        // that tab. Keep every live tab not represented in the restored set
        // (matched by groupId); the restored copy wins for any group both
        // sides agree on (its target is `{kind:"group"}`, driven by the same
        // live query the tab viewer already uses).
        setPlaybookTabs((prev) => {
          const restoredGroupIds = new Set(
            restored.map((tab) => tab.groupId).filter((groupId) => groupId.length > 0),
          );
          const liveOnly = prev.filter(
            (tab) => tab.groupId.length === 0 || !restoredGroupIds.has(tab.groupId),
          );
          return [...restored, ...liveOnly];
        });
      })
      .finally(() => {
        playbookTabsRestoreInflightRef.current = false;
        playbookTabsRestoredRef.current = true;
      });
  }, [persistedPlaybookTabsLoading, persistedPlaybookTabs]);

  // After a reload with only playbook tabs restored (no open sessions), make
  // sure one of them is active — otherwise the panel renders expanded with a
  // fully blank content area (neither viewer gate matches). Also covers the
  // general case of a tab set changing while nothing is selected. Playbook
  // tabs are external UI state relative to this restore/self-heal path (same
  // rationale as the session preferred-tab effect above).
  useEffect(() => {
    if (!playbookTabsRestoredRef.current) return;
    if (activeSessionId !== null || activePlaybookTabId !== null) return;
    const lastPlaybookTab = playbookTabs[playbookTabs.length - 1];
    // eslint-disable-next-line react-hooks/set-state-in-effect -- restores a user-owned active-tab selection persisted in settings; the selection is not derivable from render inputs and a user can legitimately clear it
    if (lastPlaybookTab) setActivePlaybookTabId(lastPlaybookTab.id);
  }, [activePlaybookTabId, activeSessionId, playbookTabs]);

  // Persist the live tab list once restore has settled — never writes back
  // while the initial restore is still in flight, or it would clobber the
  // stored list with the transitional empty array. Depends only on
  // `playbookTabs` (not the `setPersistedPlaybookTabs` setter, whose identity
  // is NOT stable across renders — `useSetting` rebuilds its query key every
  // render — which previously re-ran this effect, and therefore re-persisted
  // the setting, on every unrelated workspace render). The setter is captured
  // in a ref (synced by its own effect, never mutated during render) purely so
  // the persist effect below does not need it in its dependency array.
  const setPersistedPlaybookTabsRef = useRef(setPersistedPlaybookTabs);
  useEffect(() => {
    setPersistedPlaybookTabsRef.current = setPersistedPlaybookTabs;
  }, [setPersistedPlaybookTabs]);
  const lastPersistedPlaybookTabsJsonRef = useRef<string | null>(null);
  useEffect(() => {
    if (!playbookTabsRestoredRef.current) return;
    const next = toPersistedPlaybookTabs(playbookTabs);
    const json = JSON.stringify(next);
    if (json === lastPersistedPlaybookTabsJsonRef.current) return;
    lastPersistedPlaybookTabsJsonRef.current = json;
    void setPersistedPlaybookTabsRef.current(next);
  }, [playbookTabs]);

  useEffect(() => {
    const timers = sessionResumeTimersRef.current;
    const inflight = sessionResumeInflightRef.current;
    const attempts = sessionResumeAttemptsRef.current;
    return () => {
      for (const timer of timers.values()) {
        window.clearTimeout(timer);
      }
      timers.clear();
      inflight.clear();
      attempts.clear();
    };
  }, []);

  const resumePersistedSession = useCallback((sessionToResume: LiveSession) => {
    const sessionId = sessionToResume.id;
    if (sessionResumeInflightRef.current.has(sessionId)) return;
    if (sessionResumeTimersRef.current.has(sessionId)) return;

    sessionResumeInflightRef.current.add(sessionId);
    void resumeSession(sessionId)
      .then((response) => {
        const nextSession = liveSessionFromOpenResponse(response, {
          hostId: sessionToResume.hostId,
          hostName: sessionToResume.hostName,
          username: sessionToResume.username,
          host: hosts.find((h) => h.id === sessionToResume.hostId),
        });
        sessionResumeAttemptsRef.current.delete(sessionId);
        sessionResumeTimersRef.current.delete(sessionId);
        setSessions((prev) =>
          prev.map((session) => (session.id === sessionId ? nextSession : session)),
        );
      })
      .catch((error) => {
        const attempt = sessionResumeAttemptsRef.current.get(sessionId) ?? 0;
        if (!isTerminalSessionResumeError(error) && attempt < SESSION_RESUME_MAX_RETRIES) {
          sessionResumeAttemptsRef.current.set(sessionId, attempt + 1);
          const timer = window.setTimeout(() => {
            sessionResumeTimersRef.current.delete(sessionId);
            resumePersistedSessionRef.current(sessionToResume);
          }, sessionResumeRetryDelayMs(attempt));
          sessionResumeTimersRef.current.set(sessionId, timer);
          return;
        }

        sessionResumeAttemptsRef.current.delete(sessionId);
        sessionResumeTimersRef.current.delete(sessionId);
        setSessions((prev) =>
          prev.map((session) =>
            session.id === sessionId
              ? { ...session, status: "idle", websocketUrl: "", token: "" }
              : session,
          ),
        );
      })
      .finally(() => {
        sessionResumeInflightRef.current.delete(sessionId);
      });
  }, [hosts, setSessions]);

  useEffect(() => {
    resumePersistedSessionRef.current = resumePersistedSession;
  }, [resumePersistedSession]);

  useEffect(() => {
    const existingSessionIds = new Set(sessions.map((session) => session.id));
    for (const [sessionId, timer] of sessionResumeTimersRef.current.entries()) {
      if (!existingSessionIds.has(sessionId)) {
        window.clearTimeout(timer);
        sessionResumeTimersRef.current.delete(sessionId);
        sessionResumeAttemptsRef.current.delete(sessionId);
        sessionResumeInflightRef.current.delete(sessionId);
      }
    }

    for (const session of sessions) {
      const needsResume =
        (session.status === "connecting" || session.status === "reconnecting") &&
        !isSessionWsConnectable(session.websocketUrl, session.token);
      if (needsResume) {
        resumePersistedSession(session);
      }
    }
  }, [resumePersistedSession, sessions]);

  useEffect(() => {
    if (sessions.length === 0) {
      return;
    }
    // A playbook tab is the active selection — do not steal it back to a
    // session. Without this guard, selecting a playbook tab (which clears
    // activeSessionId) made this effect immediately re-activate a session on
    // the very next render, so both viewers ended up rendering at once.
    if (activePlaybookTabId !== null) {
      return;
    }
    if (panelPreferenceLoading && !panelStateTouchedRef.current) {
      return;
    }
    if (!panelPreferenceLoadedRef.current && !panelStateTouchedRef.current) {
      return;
    }
    if (activeSessionId && sessions.some((session) => session.id === activeSessionId)) return;
    // A saved split with no selected session is the group presentation, including
    // an empty focused pane. Preference restore must not select a standalone tab.
    if (activeSessionId === null && activePanelSession === null && persistedPanelState.paneLayout?.root.kind === "split") return;

    const preferredSession =
      findPreferredPanelSession(sessions, activePanelSession) ??
      sessions[sessions.length - 1] ??
      null;
    if (!preferredSession) return;

    const nextActiveSession = activePanelSessionFromLiveSession(preferredSession);
    // Session tabs are external UI state relative to this preference restore path.
    // eslint-disable-next-line react-hooks/set-state-in-effect -- restores the persisted preferred session into React state and the settings store together; the write-back is external, not a render-time derivation
    setActiveSessionId(preferredSession.id);
    setActivePanelSession(nextActiveSession);
    updatePersistedPanelState({ activeSession: nextActiveSession });
  }, [
    activePanelSession,
    activePlaybookTabId,
    activeSessionId,
    panelPreferenceLoading,
    persistedPanelState.paneLayout,
    sessions,
    updatePersistedPanelState,
  ]);

  // Dock visually below the catalogue only when the measured workspace cannot
  // fit both existing minima. Hidden outlets report zero; that is not geometry.
  const effectiveDockPosition: DockPosition = isMobile || (
    dockPosition === "right" && workspaceBounds.width > 0 &&
    workspaceBounds.width < CONNECTIONS_PANEL_MIN_SIZES.right +
      CONNECTIONS_WORKSPACE_MIN_MAIN_WIDTH_WITH_PANEL + CONNECTIONS_WORKSPACE_PANEL_GAP_PX
  ) ? "bottom" : dockPosition;
  // Inspector column width is independent of the docked panel and persisted.
  const { value: persistedInspectorWidth, setValue: setPersistedInspectorWidth } = useSetting<number>(
    INSPECTOR_WIDTH_SETTING_KEY,
    INSPECTOR_DEFAULT_WIDTH,
    { debounceMs: 300, normalize: clampInspectorWidth },
  );
  const [inspectorWidth, setInspectorWidth] = useState<number>(() =>
    clampInspectorWidth(persistedInspectorWidth),
  );
  const rememberInspectorWidth = useCallback(
    (width: number) => {
      const next = clampInspectorWidth(width);
      setInspectorWidth(next);
      void setPersistedInspectorWidth(next);
    },
    [setPersistedInspectorWidth],
  );
  // Apply the filter panel selection first, then the free-text search.
  const filteredByFacets = useMemo(
    () => applyFilters(hosts, filterState),
    [hosts, filterState],
  );
  const filtered = useMemo(() => {
    const q = normalizedSearch.toLowerCase();
    if (!q) return filteredByFacets;
    return filteredByFacets.filter(h => matchesTargetSearch(h, q));
  }, [filteredByFacets, normalizedSearch]);
  const tableSearchFiltered = useMemo(() => {
    const base = tableServerQueryEnabled ? applyFilters(tableHosts, filterState) : filtered;
    if (tableServerQueryEnabled) return base;
    const q = normalizedSearch.toLowerCase();
    if (!q) return base;
    return base.filter(h => matchesTargetSearch(h, q));
  }, [normalizedSearch, filterState, filtered, tableHosts, tableServerQueryEnabled]);
  const tableFiltered = useMemo(
    () =>
      applyLocalColumnFilters(tableSearchFiltered, localTableColumnFilters, (host, key) => {
        switch (key) {
          case "region":
            return host.region;
          case "tenant":
            return host.tenant;
          case "platform":
            return host.platform;
          case "deviceType":
            return host.deviceType;
          default:
            return undefined;
        }
      }),
    [localTableColumnFilters, tableSearchFiltered],
  );
  const effectiveTablePage = Math.min(tablePage, Math.max(0, Math.ceil(tableFiltered.length / tablePageSize) - 1));
  const tablePageRows = useMemo(
    () => tableFiltered.slice(effectiveTablePage * tablePageSize, (effectiveTablePage + 1) * tablePageSize),
    [tableFiltered, effectiveTablePage, tablePageSize],
  );
  const [hostLabelWindow, setHostLabelWindow] = useState<{ view: ViewMode; ids: string[] }>({ view: "tree", ids: [] });
  const lastReportedHostLabelWindowRef = useRef(hostLabelWindow);
  const reportHostLabelWindow = useCallback((view: ViewMode, ids: string[]) => {
    const previous = lastReportedHostLabelWindowRef.current;
    if (previous.view === view && previous.ids.length === ids.length && previous.ids.every((id, index) => id === ids[index])) return;
    const next = { view, ids };
    lastReportedHostLabelWindowRef.current = next;
    setHostLabelWindow(next);
  }, []);
  const reportTreeHostIds = useCallback((ids: string[]) => reportHostLabelWindow("tree", ids), [reportHostLabelWindow]);
  const reportCardHostIds = useCallback((ids: string[]) => reportHostLabelWindow("cards", ids), [reportHostLabelWindow]);
  const reportTableHosts = useCallback((rows: ConnectionTarget[]) => reportHostLabelWindow("table", rows.map((row) => row.id)), [reportHostLabelWindow]);
  const visibleHostLabelIds = useMemo(
    () => hostLabelWindow.view === viewMode ? hostLabelWindow.ids : [],
    [hostLabelWindow, viewMode],
  );
  const { byHost: hostLabelsByHost } = useHostLabels(visibleHostLabelIds);
  const handleTableColumnFilterChange = useCallback((key: string, values: string[]) => {
    setTablePage(0);
    setTableColumnFilters((current) => updateDataTableColumnFilters(current, key, values));
  }, []);
  const handleTableSortChange = useCallback((next: DataTableSortState | null) => {
    setTablePage(0);
    setTableSortState(next);
  }, []);
  const tableFilterOptions = useMemo(
    () => buildConnectionTargetFilterOptions(hosts, t),
    [hosts, t],
  );

  const hostById = useMemo(
    // Keyed by `string` so the bulk-selection `Set<string>` can look hosts up;
    // a branded `host.id: Uuid` is still a valid string key.
    () => new Map<string, ConnectionTarget>(hosts.map((host) => [host.id, host])),
    [hosts],
  );
  const effectiveSelectedHost = selectedHost ? (hostById.get(selectedHost.id) ?? selectedHost) : null;
  const effectiveSelectedNode = useMemo(() => {
    if (!selectedNode || selectedNode.type !== "host") return selectedNode;
    const freshHost = hostById.get(selectedNode.id);
    if (!freshHost) return selectedNode;
    return { ...selectedNode, name: freshHost.name, host: freshHost, hosts: [freshHost] };
  }, [hostById, selectedNode]);
  const canInspectSession = useCallback((session: LiveSession) => hostById.has(session.hostId), [hostById]);
  const inspectSession = useCallback((session: LiveSession) => {
    const host = hostById.get(session.hostId);
    if (host) inspectHost(host, true);
  }, [hostById, inspectHost]);

  // The URL retains the selected identity so editor returns and refresh can
  // restore the inspector after the workspace's host data has loaded.
  const deepLinkHost = searchParams.get(CONNECTIONS_HOST_PARAM);
  const deepLinkGroup = searchParams.get(CONNECTIONS_GROUP_PARAM);
  useEffect(() => {
    if (!visible) return;
    const identity = deepLinkHost ? `host:${deepLinkHost}` : deepLinkGroup ? `group:${deepLinkGroup}` : null;
    const group = deepLinkHost ? null : parseConnectionsGroupIdentity(deepLinkGroup);
    if (!identity || (!deepLinkHost && !group)) {
      const frame = window.requestAnimationFrame(() => {
        deepLinkAppliedRef.current = null;
        setInspectorOpen(false);
        setSelectedNode(null);
        setSelectedHost(null);
      });
      return () => window.cancelAnimationFrame(frame);
    }
    let node: TreeNode | null = null;
    if (deepLinkHost) {
      const host = hostById.get(deepLinkHost);
      if (host) node = { type: "host", id: host.id, name: host.name, host, hosts: [host] };
    } else {
      if (group) node = resolveTreeGroup(hosts, treeConfigData, group.type, group.path, group.anchor, group.entityId);
    }
    if (!node) {
      if (!selectedNode) return;
      const frame = window.requestAnimationFrame(() => {
        deepLinkAppliedRef.current = null;
        setInspectorOpen(false);
        setSelectedNode(null);
        setSelectedHost(null);
      });
      return () => window.cancelAnimationFrame(frame);
    }
    const alreadyApplied = deepLinkAppliedRef.current === identity;
    if (alreadyApplied && (node.type === "host" || node.id === selectedNode?.id)) return;
    const resolved = node;
    const frame = window.requestAnimationFrame(() => {
      deepLinkAppliedRef.current = identity;
      setSelectedNode(resolved);
      setSelectedHost(resolved.type === "host" ? resolved.host ?? null : null);
      if (!alreadyApplied) rememberInspectorOpener();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [deepLinkHost, deepLinkGroup, hosts, hostById, rememberInspectorOpener, selectedNode, treeConfigData, visible]);

  const bulkSelectedHosts = useMemo(
    () =>
      Array.from(bulkSelectedHostIds)
        .map((id) => hostById.get(id))
        .filter((host): host is ConnectionTarget => Boolean(host)),
    [bulkSelectedHostIds, hostById],
  );
  const bulkLaunchTargets = useMemo<PlaybookLaunchTargetProjection[]>(
    () => bulkSelectedHosts.map((host) => toPlaybookLaunchTarget(host, hostSubtitle([host.site, host.role, host.primaryIp]))),
    [bulkSelectedHosts],
  );
  const bulkJobsQuery = useQuery({
    queryKey: ["playbooks", "job-templates", "connections-bulk-launch"],
    queryFn: () => listJobTemplates({ limit: 100, offset: 0 }),
    enabled: bulkLaunchDialogOpen,
  });
  const bulkProjectsQuery = useQuery({
    queryKey: ["playbooks", "projects", "connections-bulk-launch"],
    queryFn: () => listProjects({ limit: 100, offset: 0 }),
    enabled: bulkLaunchDialogOpen || pendingHostLaunch !== null,
  });
  const bulkLaunchJobs = useMemo(
    () => bulkJobsQuery.data?.items ?? [],
    [bulkJobsQuery.data],
  );
  // The launch dialog checks the job's secret references in every entrypoint;
  // this ad-hoc flow queues the same job, so it loads the same list.
  const bulkSecretsQuery = useQuery({
    queryKey: AUTOMATION_SECRETS_QUERY_KEY,
    queryFn: listAutomationSecrets,
    enabled: bulkLaunchDialogOpen || pendingHostLaunch !== null,
  });
  const bulkLaunchIntent: PlaybookLaunchIntent | null = bulkLaunchDialogOpen
    ? { jobTemplateId: bulkLaunchJobId }
    : null;
  const bulkVarsSchema = useJobVarsSchema(bulkLaunchJobs.find((job) => job.id === bulkLaunchJobId), bulkLaunchDialogOpen);

  const staticSelectionLaunchMutation = useApiMutation({
    retry: false,
    mutationFn: async ({
      intent,
      targets,
      source,
    }: {
      intent: PlaybookLaunchIntent;
      targets: ConnectionTarget[];
      source: string;
    }) => {
      const job = bulkLaunchJobs.find((item) => item.id === intent.jobTemplateId);
      if (!job || job.inventory.kind !== "managed" || targets.length === 0 || !can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE") || !can("EXECUTE_NATIVE", "PLAYBOOK_JOB_TEMPLATE")) {
        throw new Error(t("playbooks.page.errors.jobTemplateRequired"));
      }
      return launchJobOnTargets(job.id, targets, source, intent.extraVars);
    },
    onSuccess: (created, variables) => {
      void queryClient.invalidateQueries({ queryKey: ["playbooks"] });
      notify("success", t("playbooks.page.toasts.runQueued", {
        count: created.run_group.total_runs,
      }), {
        source: "playbooks",
      });
      const playbookName = bulkLaunchJobs.find((job) => job.id === variables.intent.jobTemplateId)?.name
        ?? variables.intent.jobTemplateId;
      setBulkLaunchDialogOpen(false);
      setBulkSelectionMode(false);
      clearBulkSelection();
      handleRunStarted(created, playbookName);
    },
    onError: () => {
      notify("error", t("connections.bulkLaunch.launchFailed"), {
        source: "playbooks",
      });
    },
  });
  const replaceBulkSelectedHostIds = useCallback((keys: Set<string>) => {
    setBulkSelectedHostIds(new Set(keys));
  }, []);
  const clearBulkSelection = useCallback(() => {
    setBulkSelectedHostIds(new Set());
  }, []);
  const toggleBulkSelectionMode = () => {
    setBulkSelectionMode((current) => {
      if (current) {
        clearBulkSelection();
        return false;
      }
      return true;
    });
  };
  const selectVisibleBulkTargets = useCallback(() => {
    setBulkSelectionMode(true);
    setBulkSelectedHostIds((current) => {
      const next = new Set(current);
      for (const host of viewMode === "table" ? tableFiltered : filtered) {
        next.add(host.id);
      }
      return next;
    });
  }, [filtered, tableFiltered, viewMode]);
  const handleBulkLaunchSelectedTargets = () => {
    if (bulkSelectedHosts.length === 0) {
      notify("warning", t("connections.bulkLaunch.noSelection"));
      return;
    }
    staticSelectionLaunchMutation.reset();
    setBulkLaunchDialogOpen(true);
  };

  const submitBulkLaunch = useCallback((intent: PlaybookLaunchIntent) => {
    if (bulkSelectedHosts.length === 0) return;
    staticSelectionLaunchMutation.mutate({
      intent,
      targets: bulkSelectedHosts,
      source: "connections.bulk_launch",
    });
  }, [bulkSelectedHosts, staticSelectionLaunchMutation]);

  const sessionLaunch = useSessionLaunch<WorkspaceLaunchDestination>({
    isCurrent: destination => destination.kind === "new" || sessions.some(item => item.id === destination.previous.id),
    prepare: destination => destination.kind === "new" && destination.openInNewTab && (!destination.reservedWindow || destination.reservedWindow.closed)
      ? { ...destination, reservedWindow: reserveDetachedBrowserWindow() } : destination,
    discard: destination => { if (destination.kind === "new") closeReservedBrowserWindow(destination.reservedWindow); },
    accept: (response, destination, username) => {
      const host = destination.kind === "replace" ? hosts.find(item => item.id === destination.previous.hostId) : destination.host;
      const next = liveSessionFromOpenResponse(response, {
        hostId: response.session.hostId,
        hostName: destination.kind === "replace" ? destination.previous.hostName : destination.host.name,
        username, host,
      });
      if (destination.kind === "replace") {
        const oldId = destination.previous.id;
        if (!sessions.some(item => item.id === oldId)) return false;
        if (activeSessionId === oldId) focusSession();
        setSessions(current => current.map(item => item.id === oldId ? next : item));
        setActiveSessionId(current => current === oldId ? next.id : current);
        setActivePanelSession(current => current?.sessionId === oldId ? activePanelSessionFromLiveSession(next) : current);
        void setPersistedPanelState(current => ({
          ...current,
          activeSession: current.activeSession?.sessionId === oldId ? activePanelSessionFromLiveSession(next) : current.activeSession,
          ...(current.paneLayout ? { paneLayout: replacePaneSession(current.paneLayout, oldId, next.id) } : {}),
        }));
        return true;
      }
      if (destination.openInNewTab && openDetachedSession(next, destination.reservedWindow, () => attachPanelSession(next))) {
        dismissInspector();
        return true;
      }
      attachPanelSession(next);
      return true;
    },
  });

  const closeSessionMutation = useApiMutation({
    mutationFn: (sessionId: string) => apiCloseSession(sessionId),
  });

  // Open (or focus) the device-management console as a client-only tab. It
  // serves both a video recorder (CCTV entry) and a Linux host (neutral Accounts
  // entry): the panel drives the device REST plane directly, with no backend
  // session, websocket, or token, so it never goes through the session
  // create/resume/teardown machinery. The id is stable per host so reopening
  // focuses the existing tab instead of stacking duplicates.
  const openDeviceConsole = useCallback(
    (host: ConnectionTarget) => {
      void preloadLiveSessionProtocol("cctv").catch(() => {});
      const tab: LiveSession = {
        id: `cctv:${host.id}`,
        hostId: host.id,
        hostName: host.name,
        protocol: "cctv",
        status: "connected",
        websocketUrl: "",
        token: "",
        hostRole: host.role,
        hostSite: host.site,
        hostRegion: host.region,
        hostTenant: host.tenant,
        hostCluster: host.cluster,
      };
      attachPanelSession(tab);
    },
    [attachPanelSession],
  );

  const openSession = useCallback(
    (
      host: ConnectionTarget,
      protocol: "ssh" | "rdp" | "vnc" | "cctv",
      entryId?: string,
      username?: string,
    ) => {
      if (protocol === "cctv") {
        openDeviceConsole(host);
        return;
      }
      if (protocol === "vnc") {
        void preloadLiveSessionProtocol("vnc").catch(() => {
          // Keep connect authoritative; preload only moves chunk evaluation out
          // of the post-API attach path in slower desktop WebViews.
        });
      }
      const openInNewTab = shouldOpenConnectionInNewTab(openInNewTabSetting, protocol);
      const reservedWindow = openInNewTab ? reserveDetachedBrowserWindow() : null;
      // Reuse an existing session for the same host+protocol (only when no specific account is chosen).
      if (!entryId) {
        const existing = sessions.find((s) => s.hostId === host.id && s.protocol === protocol);
        if (existing) {
          if (
            openInNewTab &&
            openDetachedSession(
              existing,
              reservedWindow,
              () => activatePanelSession(existing, { showPanel: true, expanded: true }),
              () => removeLocalSession(existing.id),
            )
          ) {
            dismissInspector();
            return;
          }
          closeReservedBrowserWindow(reservedWindow);
          activatePanelSession(existing, { showPanel: true, expanded: true });
          return;
        }
      }
      void sessionLaunch.launch({ hostId: host.id, protocol, entryId }, { kind: "new", host, openInNewTab, reservedWindow }, username).catch((error: unknown) => {
        closeReservedBrowserWindow(reservedWindow);
        const description = error instanceof Error && "reasonCode" in error && error.reasonCode === "endpoint_not_bound_to_target"
          ? t("connections.connectErrorNoBoundAddress") : error instanceof Error ? error.message : "";
        notify("error", t("connections.connectionFailed"), { description });
      });
    },
    [
      activatePanelSession,
      dismissInspector,
      openDeviceConsole,
      openDetachedSession,
      openInNewTabSetting,
      sessionLaunch,
      removeLocalSession,
      sessions,
      t,
    ],
  );

  const setAdHocContext = useCallback((context: AdHocCredentialsContext) => {
    const host = hosts.find(item => item.id === context.host.id);
    if (!host) return;
    sessionLaunch.openCredentials(context, {
      kind: "new", host,
      openInNewTab: shouldOpenConnectionInNewTab(openInNewTabSetting, context.protocol),
      reservedWindow: null,
    });
  }, [hosts, openInNewTabSetting, sessionLaunch]);

  const handleRunStarted = (
    created: PlaybookRunGroupCreated,
    playbookName: string,
  ) => {
    dismissInspector();
    setShowFilters(false);
    openPlaybookTab(
      { kind: "seed", runs: created.runs, groupId: created.run_group.id },
      playbookName,
    );
  };

  const hostJobLaunch = useApiMutation({
    retry: false,
    mutationFn: ({ host, item, extraVars }: { host: ConnectionTarget; item: PlaybookTargetAssignment; extraVars?: Extravars }) => {
      if (!can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE") || !can("EXECUTE_NATIVE", "PLAYBOOK_JOB_TEMPLATE") || !item.assignment.enabled || !item.assignment.executable || item.assignment.job_template_id !== item.job_template.id || item.job_template.inventory.kind !== "managed") {
        throw new Error(t("playbooks.page.errors.jobTemplateRequired"));
      }
      return launchJobOnTargets(item.assignment.job_template_id, [host], "connections.host_assignment", extraVars);
    },
    onSuccess: (created, { item }) => {
      setPendingHostLaunch(null);
      void queryClient.invalidateQueries({ queryKey: ["playbooks"] });
      handleRunStarted(created, item.assignment.name || item.job_template.name);
    },
    onError: (error) => notify("error", t([400, 422].includes(error.status)
      ? "connections.bulkLaunch.selectionRejected"
      : error.status === 403 ? "connections.bulkLaunch.forbidden" : "connections.bulkLaunch.launchFailed"), { source: "playbooks" }),
  });
  const launchHostJob = useCallback(async (host: ConnectionTarget, item: PlaybookTargetAssignment) => {
    try {
      const varsSchema = item.job_template.vars_schema?.kind === "project_file" ? await queryClient.fetchQuery({ queryKey: jobVarsSchemaQueryKey(item.job_template), queryFn: () => getJobTemplateVarsSchema(item.job_template.id) }) : undefined;
      const schema = jobVarsSchema(item.job_template.vars_schema, varsSchema);
      const values = effectiveParameters(schema, parseParameterValues(JSON.stringify(item.job_template.options.extra_vars)), {});
      if (missingRequiredParameters(schema, values).length) { setPendingHostLaunch({ host, item, varsSchema }); return; }
      hostJobLaunch.mutate({ host, item });
    } catch { notify("error", t("playbooks.parameters.schemaFailed"), { source: "playbooks" }); }
  }, [hostJobLaunch, queryClient, t]);


  const renderTableQuickAccess = useCallback(
    (host: ConnectionTarget) => (
      <div className="flex flex-nowrap items-center gap-1">
        <Button
          variant="ghost"
          size="sm"
          className="shrink-0 whitespace-nowrap"
          onClick={(event) => { event.stopPropagation(); inspectHost(host); }}
        >
          {t("connections.details")}
        </Button>
        <QuickAccessButtons
          host={host}
          config={treeConfigData.quickAccess}
          onConnect={openSession}
          onPlaybookQuickAction={launchHostJob}
          variant="table"
          intentStartedAt={tableIntentStartedAt(host.id)}
          onIntent={() => markTableIntent(host.id)}
        />
      </div>
    ),
    [
      inspectHost,
      openSession,
      launchHostJob,
      treeConfigData.quickAccess,
      tableIntentStartedAt,
      markTableIntent,
      t,
    ],
  );

  const closeSession = useCallback(
    (sessionId: string) => {
      // A CCTV console tab has no backend session — only drop the local tab; a
      // `disconnect` POST would 404. Proxied sessions get the graceful close.
      const session = sessions.find((candidate) => candidate.id === sessionId);
      if (session?.protocol !== "cctv") {
        closeSessionMutation.mutate(sessionId);
      }
      removeLocalSession(sessionId);
    },
    [closeSessionMutation, removeLocalSession, sessions],
  );

  const closeAllSessions = useCallback(() => {
    const proxiedSessionIds = sessions
      .filter((session) => session.protocol !== "cctv")
      .map((session) => session.id);
    setSessions([]);
    rememberActivePanelSession(null);
    hidePanelOrActivateRemainingPlaybookTab();
    for (const sessionId of proxiedSessionIds) {
      apiCloseSession(sessionId).catch(() => {
        // Viewer unmount already closes the relay; stale/disconnected sessions are harmless here.
      });
    }
  }, [hidePanelOrActivateRemainingPlaybookTab, rememberActivePanelSession, sessions, setSessions]);

  const reconnectSession = useCallback(
    (sessionToReconnect: LiveSession) => {
      if (sessionToReconnect.protocol === "cctv") {
        // Client-only console: there is no backend session to close/resume —
        // "reconnect" just refetches every device query the console renders.
        void queryClient.invalidateQueries({
          queryKey: ["device", sessionToReconnect.hostId],
        });
        setSessions((prev) =>
          prev.map((session) =>
            session.id === sessionToReconnect.id
              ? { ...session, status: "connected" as const }
              : session,
          ),
        );
        return;
      }
      void sessionLaunch.retry({ ...sessionToReconnect, protocol: sessionToReconnect.protocol }, { kind: "replace", previous: sessionToReconnect });
    },
    [queryClient, sessionLaunch, setSessions],
  );

  const detachSession = useCallback((sessionId: string) => {
    const session = sessions.find(s => s.id === sessionId);
    if (!session) return;
    const opened = openDetachedSession(
      session,
      null,
      undefined,
      () => removeLocalSession(sessionId),
    );
    if (!opened) return;
    // Relinquish-first: the backend proxy guards a connection against concurrent
    // attach, so the detached window can only take over once the in-panel WS
    // releases the lock. Drop the local session immediately (viewer unmount
    // closes the socket); the detached window adopts the stored handoff and, if
    // it briefly loses the attach race, re-resumes for a fresh token.
  }, [openDetachedSession, removeLocalSession, sessions]);

  /**
   * Open EVERY open session in a single detached window, preserving their tab
   * strip order. Re-uses the per-session handoff (writes one localStorage entry
   * per session like single-detach does) and additionally injects the array on
   * the new window so the group page can boot without a localStorage round-trip.
   * Same relinquish-first rationale as single-detach: clear the in-panel state
   * once the window is open so the detached window owns the WS attach.
   */
  const detachAllSessions = useCallback(() => {
    if (sessions.length === 0) return;
    const handoffs: DetachedHandoff[] = sessions.map((session) => ({
      sessionId: session.id,
      hostName: session.hostName,
      hostId: session.hostId,
      protocol: session.protocol,
      websocketUrl: session.websocketUrl,
      token: session.token,
      ...(session.username ? { username: session.username } : {}),
    }));
    for (const handoff of handoffs) {
      writeDetachedHandoff(handoff.sessionId, handoff);
    }
    const ids = sessions.map((s) => encodeURIComponent(s.id)).join(",");
    const detachedPath = `/connections/detached-group?ids=${ids}`;
    const detachedWindow = window.open(detachedPath, "_blank");
    if (!detachedWindow) {
      notify("warning", t("connections.detachSession"), {
        description: t("connections.popupBlocked"),
        source: "connections",
      });
      return;
    }
    try {
      detachedWindow.__PIPING_DETACHED_GROUP_HANDOFF__ = handoffs;
    } catch {
      // Cross-origin shouldn't happen for our own route, but if it does the
      // group page still recovers from per-session localStorage handoffs.
    }
    setSessions([]);
    rememberActivePanelSession(null);
    hidePanelOrActivateRemainingPlaybookTab();
  }, [hidePanelOrActivateRemainingPlaybookTab, rememberActivePanelSession, sessions, setSessions, t]);

  const columnByKey = useMemo<Record<ConnectionTargetsTableColumnKey, Column<ConnectionTarget>>>(
    () => ({
      name: {
        key: "name",
        label: t("connections.columns.name"),
        sortable: true,
        getSortValue: (r) => r.name,
        render: (r) => (
            <div className="flex items-center gap-2">
              <EntityIcon
                fallback={connectionTargetFallbackIcon(r)}
                iconName={r.roleIconName}
                customDataUrl={r.roleIconDataUrl}
                className="h-6 w-6 rounded-md bg-accent text-primary"
                iconClassName="h-3.5 w-3.5"
              />
              <HostStatusDot status={r.status} />
              <span className="min-w-0">
                <span className="block max-w-48 truncate font-medium text-foreground" title={r.name}>
                  {r.name}
                </span>
                <span className="block max-w-48 truncate text-xs text-muted-foreground">
                  {hostSubtitle([r.platform, r.deviceType])}
                </span>
              </span>
              <TargetBindingWarning host={r} />
              <HostObservabilityCluster entry={hostLabelsByHost.get(r.id)} />
            </div>
          ),
      },
      status: {
        key: "status",
        label: t("connections.columns.status"),
        sortable: true,
        filterOptions: tableFilterOptions.status,
        getSortValue: (r) => r.status ?? "",
        render: (r) => (
          <div className="flex flex-col items-start gap-1">
            <div className="flex items-center gap-1">
              <span className="text-xs text-muted-foreground">{t("connections.inventoryStatus")}</span>
              <HostStatusValue status={r.status} label={inventoryResourceStatusLabel(t, r.status)} />
            </div>
            <div className="flex items-center gap-1">
              <span className="text-xs text-muted-foreground">{t("connections.monitoringStatus")}</span>
              <HostAvailabilityIndicator entry={hostLabelsByHost.get(r.id)} />
            </div>
          </div>
        ),
      },
      targetKind: {
        key: "targetKind",
        label: t("connections.columns.type"),
        sortable: true,
        filterOptions: tableFilterOptions.targetKind,
        getSortValue: (r) => r.targetKind,
        render: (r) =>
          hostValue(
            r.targetKind === "device"
              ? t("connections.targetKind.device", "Device")
              : t("connections.targetKind.virtualMachine", "Virtual machine"),
          ),
      },
      region: {
        key: "region",
        label: t("connections.columns.region"),
        sortable: true,
        filterOptions: tableFilterOptions.region,
        getSortValue: (r) => r.region ?? "",
        render: (r) => hostValue(r.region),
      },
      site: {
        key: "site",
        label: t("connections.columns.site"),
        sortable: true,
        filterOptions: tableFilterOptions.site,
        getSortValue: (r) => r.site ?? "",
        render: (r) => hostValue(r.site),
      },
      tenant: {
        key: "tenant",
        label: t("connections.columns.tenant"),
        sortable: true,
        filterOptions: tableFilterOptions.tenant,
        getSortValue: (r) => r.tenant ?? "",
        render: (r) => hostValue(r.tenant),
      },
      role: {
        key: "role",
        label: t("connections.columns.role"),
        sortable: true,
        filterOptions: tableFilterOptions.role,
        getSortValue: (r) => r.role ?? "",
        render: (r) => hostValue(r.role),
      },
      platform: {
        key: "platform",
        label: t("connections.columns.platform"),
        sortable: true,
        filterOptions: tableFilterOptions.platform,
        getSortValue: (r) => r.platform ?? "",
        render: (r) => hostValue(r.platform),
      },
      deviceType: {
        key: "deviceType",
        label: t("connections.columns.deviceType", "Device type"),
        sortable: true,
        filterOptions: tableFilterOptions.deviceType,
        getSortValue: (r) => r.deviceType ?? "",
        render: (r) => hostValue(r.deviceType),
      },
      primaryIp: {
        key: "primaryIp",
        label: t("connections.columns.primaryIp"),
        sortable: true,
        getSortValue: (r) => r.primaryIp ?? "",
        render: (r) => (
          <span className="font-mono text-xs text-muted-foreground">{hostValue(r.primaryIp)}</span>
        ),
      },
      actions: {
        key: "actions",
        label: "",
        excludeFromSettings: true,
        render: (r) => renderTableQuickAccess(r),
      },
    }),
    [renderTableQuickAccess, t, tableFilterOptions, hostLabelsByHost],
  );
  const columns = useMemo(
    () => CONNECTION_TARGETS_TABLE_CONFIG.viewColumns.map((key) => columnByKey[key]),
    [columnByKey],
  );

  const hasOpenTabs = sessions.length > 0 || playbookTabs.length > 0;
  const sideSessionWidth = effectiveDockPosition === "right" && showPanel && hasOpenTabs
    ? (panelExpanded ? Math.max(CONNECTIONS_PANEL_MIN_SIZES.right, panelSizes.right) : 36)
    : 0;
  const inspectorRequiredWidth = inspectorWidth + 6 + CONNECTIONS_WORKSPACE_MIN_MAIN_WIDTH_WITH_PANEL +
    CONNECTIONS_WORKSPACE_PANEL_GAP_PX + (sideSessionWidth > 0 ? sideSessionWidth + CONNECTIONS_WORKSPACE_PANEL_GAP_PX : 0);
  const inspectorOverlay = isMobile || (isMobileSession && showPanel && hasOpenTabs) ||
    workspaceBounds.width < inspectorRequiredWidth;
  const inspectorConsumesWidth = inspectorOpen && !inspectorOverlay
    && (viewMode === "tree" ? !!effectiveSelectedNode : !!effectiveSelectedHost);
  const inlineInspectorWidth = inspectorConsumesWidth ? inspectorWidth + 6 : 0;
  const panelAvailableSize = useMemo(() => {
    if (effectiveDockPosition === "right") {
      return workspaceBounds.width -
        inlineInspectorWidth -
        CONNECTIONS_WORKSPACE_MIN_MAIN_WIDTH_WITH_PANEL -
        CONNECTIONS_WORKSPACE_PANEL_GAP_PX * (inspectorConsumesWidth ? 2 : 1);
    }

    return workspaceBounds.height -
      CONNECTIONS_WORKSPACE_MIN_MAIN_HEIGHT_WITH_PANEL -
      CONNECTIONS_WORKSPACE_PANEL_GAP_PX;
  }, [effectiveDockPosition, inlineInspectorWidth, inspectorConsumesWidth, workspaceBounds.height, workspaceBounds.width]);
  const panelSize = clampConnectionsPanelSize(
    effectiveDockPosition,
    panelSizes[effectiveDockPosition],
    panelAvailableSize,
  );
  const isBottomDock = effectiveDockPosition === "bottom";
  const openTabsCount = sessions.length + playbookTabs.length;
  const openPlaybookTabGroupIds = useMemo(
    () => new Set(playbookTabs.map((tab) => tab.groupId).filter((groupId) => groupId.length > 0)),
    [playbookTabs],
  );

  const searchActive = normalizedSearch.length > 0;
  const { can } = usePermissions();
  const canAddTarget = can("READ", "INVENTORY") && can("CREATE", "INVENTORY");
  const catalogueQueries = tableServerQueryEnabled ? [hostsQuery, tableHostsQuery] : [hostsQuery];
  // A query disabled by the current view keeps its old error state; it must not
  // keep the banner up or swallow the retry click.
  const activeCatalogueQueries = catalogueQueries.filter(query => query.isEnabled);
  const failedCatalogueQueries = activeCatalogueQueries.filter(query => query.isError);
  const catalogueUnavailable = activeCatalogueQueries.some(query => query.data === undefined);
  const catalogueLoading = catalogueUnavailable && failedCatalogueQueries.length === 0;
  const catalogueError = failedCatalogueQueries.length > 0;
  const catalogueHasConstraints = searchActive || activeFilterCount > 0 ||
    (viewMode === "table" && Object.values(sanitizedTableColumnFilters).some(values => values.length > 0));
  const resetCatalogue = () => {
    setSearchQuery("");
    setFilterState(deserializeFilterState({}));
    setTableColumnFilters({});
    searchInputRef.current?.focus();
  };
  const retryCatalogue = async () => {
    if (catalogueRetrying) return;
    setCatalogueRetrying(true);
    try {
      await Promise.allSettled(
        failedCatalogueQueries.map(query => query.refetch({ cancelRefetch: true })),
      );
    } finally {
      setCatalogueRetrying(false);
    }
  };
  const catalogueFeedback = catalogueError ? (
    <div role="alert" className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-muted p-4 text-sm">
      <p>{t(catalogueUnavailable ? "connections.catalogue.loadError" : "connections.catalogue.refreshError")}</p>
      <Button variant="outline" size="sm" onClick={() => { void retryCatalogue(); }} disabled={catalogueRetrying}>
        {catalogueRetrying ? t("connections.catalogue.retrying") : t("connections.catalogue.retry")}
      </Button>
    </div>
  ) : catalogueLoading ? (
    <div role="status" className="flex items-center justify-center gap-2 p-12 text-sm text-muted-foreground">
      <Loader2 className="size-4 animate-spin" aria-hidden="true" />{t("connections.catalogue.loading")}
    </div>
  ) : null;
  const catalogueEmpty = (
    <div className="flex flex-col items-center gap-4 py-8 text-center">
      <EmptyState icon={Search} className="py-0"
        title={t(catalogueHasConstraints ? "connections.catalogue.noMatch" : "connections.catalogue.empty")}
        description={t(catalogueHasConstraints ? "connections.tryAdjusting" : "connections.catalogue.emptyDescription")} />
      {catalogueHasConstraints ? (
        <Button variant="outline" onClick={resetCatalogue}>{t("connections.catalogue.reset")}</Button>
      ) : canAddTarget ? (
        <div className="flex flex-wrap justify-center gap-2">
          <Button asChild><Link to={inventoryResourceNewPath("Devices", "/connections")}>{t("connections.catalogue.addDevice")}</Link></Button>
          <Button variant="outline" asChild><Link to={inventoryResourceNewPath("Virtual Machines", "/connections")}>{t("connections.catalogue.addVm")}</Link></Button>
        </div>
      ) : <p className="max-w-sm px-4 text-sm text-muted-foreground">{t("connections.catalogue.permission")}</p>}
    </div>
  );

  const renderToolbar = () => (
    <div
      className="@container flex w-full min-w-0 flex-col gap-2"
      // The single toolbar row owns its own container query: the catalog column
      // narrows when a session panel docks beside it, and the view labels must
      // collapse to icons on the container's width, not the viewport's.
    >
      <div className="flex w-full min-w-0 items-center gap-2">
        {!persistedLoading && (
          <div role="group" aria-label={t("connections.viewSelection")} className="flex shrink-0 items-center gap-1.5">
            {(["tree", "cards", "table", "map"] as ViewMode[]).filter(mode => mode !== "map" || can("READ", "INVENTORY")).map(mode => {
              const label = t(`connections.views.${mode}`);
              return <TogglePill key={mode} active={viewMode === mode} onClick={() => setViewMode(mode)} shape="md"
                icon={<ViewModeIcon mode={mode} />} title={label} ariaLabel={label} className="px-2 @3xl:px-3">
                <span className="hidden @3xl:inline">{label}</span>
              </TogglePill>;
            })}
          </div>
        )}
        <div className="min-w-0 flex-1">
          <SearchInput
            ref={searchInputRef}
            value={searchQuery}
            onValueChange={(value) => { setTablePage(0); setSearchQuery(value); }}
            onClear={clearSearch}
            clearLabel={t("connections.clearSearch")}
            placeholder={isMobile ? t("connections.searchShort") : t("connections.searchPlaceholder")}
            containerClassName="sm:w-full"
            className="pl-8 focus:bg-accent"
          />
        </div>
        {persistedLoading ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground shrink-0" />
        ) : (
          <>
            <Button variant="ghost" size="sm"
              ref={filtersButtonRef}
              type="button"
              onClick={() => setShowFilters(!showFilters)}
              title={t("connections.filters")}
              // Without this the accessible name is computed from the content —
              // an icon with no text plus the count badge — so a screen reader
              // announces the button as just "2". The label carries the purpose
              // AND the count, and the badge below is hidden from the tree so it
              // is not read twice.
              aria-label={
                activeFilterCount > 0
                  ? t("connections.filtersActive", {
                      count: activeFilterCount,
                      defaultValue: "Filters ({{count}} active)",
                    })
                  : t("connections.filters")
              }
              aria-expanded={showFilters}
              className={filterButtonClass(activeFilterCount, showFilters)}
            >
              <Filter className="h-3.5 w-3.5" />
              {activeFilterCount > 0 && (
                <span
                  aria-hidden="true"
                  className="absolute -right-1 -top-1 flex h-4 min-w-4 items-center justify-center rounded-full border border-background bg-primary px-1 text-xs font-semibold leading-none text-primary-foreground shadow-sm"
                >
                  {activeFilterCount > 99 ? "99+" : activeFilterCount}
                </span>
              )}
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="icon" className="shrink-0" aria-label={t("connections.catalogueActions")} title={t("connections.catalogueActions")}>
                  <Settings className="size-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={toggleBulkSelectionMode}><CheckSquare className="size-4" />{t(bulkSelectionMode ? "connections.bulkLaunch.exitSelection" : "connections.bulkLaunch.enterSelection")}</DropdownMenuItem>
                {viewMode === "tree" && <DropdownMenuItem onSelect={() => setTreeSettingsOpen(true)} disabled={treeConfigLoading}><Settings className="size-4" />{t("connections.treeSettings.title")}</DropdownMenuItem>}
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        )}
      </div>
      {!persistedLoading && bulkSelectionMode && (
        <div className="flex w-full min-w-0 flex-wrap items-center gap-1 rounded-md border border-border bg-card px-1.5 py-1 text-xs text-muted-foreground">
          <span className="shrink-0 px-1 font-medium text-foreground/80">
            {t("connections.bulkLaunch.selectedCount", { count: bulkSelectedHosts.length })}
          </span>
          <Button variant="ghost" size="sm"
            type="button"
            onClick={selectVisibleBulkTargets}
            className="h-9 rounded px-3 text-sm font-medium hover:bg-accent hover:text-foreground"
          >
            {t("connections.bulkLaunch.selectVisible")}
          </Button>
          <Button variant="ghost" size="sm"
            type="button"
            onClick={handleBulkLaunchSelectedTargets}
            disabled={bulkSelectedHosts.length === 0}
            className="inline-flex h-9 items-center gap-2 rounded bg-primary px-3 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-45"
          >
            <PlayCircle className="h-3 w-3" />
            <span>{t("connections.bulkLaunch.launchSelected")}</span>
          </Button>
          <Button variant="ghost" size="sm"
            type="button"
            onClick={clearBulkSelection}
            disabled={bulkSelectedHosts.length === 0}
            aria-label={t("connections.bulkLaunch.clearSelection")}
            title={t("connections.bulkLaunch.clearSelection")}
            className="inline-flex h-9 w-9 items-center justify-center rounded hover:bg-accent hover:text-foreground disabled:cursor-not-allowed disabled:opacity-45"
          >
            <X className="h-3 w-3" />
          </Button>
        </div>
      )}
    </div>
  );

  /** One frame for every view: the toolbar row and the content inset must not
      differ between tree, cards, table and map. */
  const catalogueFrame = (content: ReactNode) => (
    <div className="flex min-h-0 flex-1 flex-col gap-4">
      <div className="flex w-full min-w-0 shrink-0 items-center">
        {renderToolbar()}
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-4">
        {content}
      </div>
    </div>
  );

  const renderMainContent = () => {
    if (catalogueUnavailable && viewMode !== "table") {
      return catalogueFrame(catalogueFeedback);
    }
    if (viewMode === "map") {
      return catalogueFrame(<>
        {catalogueFeedback}
        {!can("READ", "INVENTORY") ? <div role="status" className="p-8 text-center text-sm text-muted-foreground">{t("connections.map.permission")}</div> : filtered.length === 0 ? catalogueEmpty :
          <Suspense fallback={<div role="status" className="p-8 text-sm text-muted-foreground">{t("connections.map.loading")}</div>}><WorkspaceMap targets={filtered} selectedTarget={effectiveSelectedHost} onSelect={inspectHost} renderActions={host => <QuickAccessButtons host={host} config={treeConfigData.quickAccess} onConnect={openSession} onPlaybookQuickAction={launchHostJob} variant="table" />} /></Suspense>}
      </>);
    }
    if (viewMode === "table") {
      return catalogueFrame(<>
        {catalogueFeedback}
        <DataTable
          tableId={CONNECTION_TARGETS_TABLE_CONFIG.tableId}
          onRenderedRowsChange={reportTableHosts}
          columns={columns} data={tablePageRows} getRowKey={r => r.id}
          selectedRow={effectiveSelectedHost} onRowClick={(row) => inspectHost(row)} className="flex-1"
          onRowIntent={(row) => markTableIntent(row.id)}
          sortState={sanitizedTableSortState}
          onSortChange={handleTableSortChange}
          columnFilters={sanitizedTableColumnFilters}
          onColumnFilterChange={handleTableColumnFilterChange}
          selectable={bulkSelectionMode}
          selectedKeys={bulkSelectedHostIds}
          onSelectionChange={(keys) => {
            const pageIds = new Set<string>(tablePageRows.map((row) => row.id));
            const merged = new Set([...bulkSelectedHostIds].filter((id) => !pageIds.has(id)));
            for (const id of keys) if (pageIds.has(id)) merged.add(id);
            replaceBulkSelectedHostIds(merged);
          }}
          virtualizeRows={{ threshold: 100, estimateSize: 37, overscan: 16 }}
          pagination={{ page: effectiveTablePage, pageSize: tablePageSize, total: tableFiltered.length, onPageChange: setTablePage, onPageSizeChange: (size) => { setTablePageSize(size); setTablePage(0); } }}
          emptyState={catalogueUnavailable || catalogueError ? <></> : catalogueEmpty}
        />
      </>);
    }
    if (viewMode === "cards") {
      return catalogueFrame(<>
        {catalogueFeedback}
        {filtered.length === 0 ? (catalogueError ? null : catalogueEmpty) : (
          <CardsView
            onRenderedHostIdsChange={reportCardHostIds}
            hosts={filtered}
            selectedHost={effectiveSelectedHost}
            onSelectHost={inspectHost}
            revealHostId={visible && effectiveSelectedHost?.id === deepLinkHost ? deepLinkHost : null}
            revealNavigationKey={revealNavigationKey}
            onConnect={openSession}
            onPlaybookQuickAction={launchHostJob}
            quickAccessConfig={treeConfig.config.quickAccess}
            bulkSelectionMode={bulkSelectionMode}
            bulkSelectedHostIds={bulkSelectedHostIds}
            onBulkSelectionChange={replaceBulkSelectedHostIds}
            hostLabels={hostLabelsByHost}
          />
        )}
      </>);
    }
    return catalogueFrame(<>
      {catalogueFeedback}
      {filtered.length === 0 ? (catalogueError ? null : catalogueEmpty) : (
        <TreeView
          onRenderedHostIdsChange={reportTreeHostIds}
          hosts={filtered}
          selectedHost={effectiveSelectedHost}
          onSelectHost={inspectHost}
          revealHostId={visible && effectiveSelectedHost?.id === deepLinkHost ? deepLinkHost : null}
          revealNavigationKey={revealNavigationKey}
          revealReady={!treeConfigLoading}
          onConnect={openSession}
          onPlaybookQuickAction={launchHostJob}
          selectedNode={effectiveSelectedNode}
          onSelectNode={inspectNode}
          config={treeConfig.config}
          bulkSelectionMode={bulkSelectionMode}
          bulkSelectedHostIds={bulkSelectedHostIds}
          onBulkSelectionChange={replaceBulkSelectedHostIds}
          searchActive={searchActive}
          hostLabels={hostLabelsByHost}
        />
      )}
    </>);
  };

  const handlePaneLayoutChange = useCallback(
    (paneLayout: SessionPaneLayout) => updatePersistedPanelState({ paneLayout }),
    [updatePersistedPanelState],
  );

  const handleToggleExpand = useCallback(() => {
    if (isMobileSession) {
      dismissInspector();
      rememberPanelVisibility(false);
      return;
    }
    if (!panelExpanded) focusSession();
    rememberPanelExpanded(!panelExpanded);
  }, [
    focusSession,
    dismissInspector,
    isMobileSession,
    panelExpanded,
    rememberPanelExpanded,
    rememberPanelVisibility,
  ]);

  const inspectorTarget = viewMode === "tree" ? effectiveSelectedNode : effectiveSelectedHost;
  const inspector = inspectorOpen && inspectorTarget ? (
    <InspectorPane
      width={inspectorWidth}
      onResize={rememberInspectorWidth}
      overlay={inspectorOverlay}
      mobile={isMobile}
      onClose={closeInspector}
      title={inspectorTarget.name}
      outsideFilter={!!effectiveSelectedHost && !filtered.some(host => host.id === effectiveSelectedHost.id)}
    >
      {viewMode === "tree" && effectiveSelectedNode ? (
        <TreeNodeInspector
          node={effectiveSelectedNode}
          onClose={closeInspector}
          onConnect={openSession}
          onPlaybookQuickAction={launchHostJob}
          onOpenAdHoc={setAdHocContext}
          stacked
        />
      ) : effectiveSelectedHost ? (
        <ConnectionInspector
          host={effectiveSelectedHost}
          onClose={closeInspector}
          onConnect={openSession}
          onPlaybookQuickAction={launchHostJob}
          onOpenAdHoc={setAdHocContext}
          stacked
        />
      ) : null}
    </InspectorPane>
  ) : null;

  const renderWorkspace = () => (
    <div ref={workspaceRef} className={workspaceRootClass(isMobile, isBottomDock)}>
      <PageChrome title={t("routeTitles./connections")} visible={visible} />
      <div
        style={!isMobile && !isBottomDock && showPanel && hasOpenTabs ? { display: "contents" } : undefined}
        className={mainAreaClass(isMobileSession, showPanel, hasOpenTabs, isMobile, isBottomDock)}
      >
        <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          {renderMainContent()}
        </div>
        {inspector}
      </div>

      {/* Catalog filters — one Radix dialog on every viewport. The list stays
          visible behind it; the draft only reaches the list on Apply. */}
      <Dialog open={showFilters} onOpenChange={setShowFilters}>
        <DialogContent
          onCloseAutoFocus={event => { event.preventDefault(); filtersButtonRef.current?.focus(); }}
          className="surface flex max-h-[85dvh] w-[min(42rem,calc(100vw-2rem))] flex-col overflow-hidden border-border/40 p-0 sm:max-w-2xl"
          description={t("connections.filtersPanel.dialogDescription")}
        >
          <ConnectionsFilters
            onClose={() => setShowFilters(false)}
            hosts={hosts}
            selected={filterState}
            onChange={setFilterState}
            privatePresets={privateFilterPresets}
            publicPresets={publicFilterPresets}
            presetsLoading={privateFilterPresetsLoading || publicFilterPresetsQuery.isLoading}
            onSavePreset={saveFilterPreset}
            onApplyPreset={applyFilterPreset}
            onDeletePreset={deleteFilterPreset}
            savingPreset={createPublicPresetMutation.isPending}
            deletingPresetId={deletingPublicPresetId}
          />
        </DialogContent>
      </Dialog>

      {/* Connections panel — session tabs + playbook run tabs in one strip */}
      {hasOpenTabs && (
        <div className={showPanel ? "contents" : "hidden"}>
          <ConnectionsPanel
            mobile={isMobileSession}
            visible={showPanel && visible}
            sessions={sessions}
            activeSessionId={activeSessionId}
            paneLayout={persistedPanelState.paneLayout}
            onPaneLayoutChange={handlePaneLayoutChange}
            onSelectSession={selectPanelSession}
            onInspectSession={inspectSession}
            canInspectSession={canInspectSession}
            focusRequest={sessionFocusRequest}
            onCloseSession={closeSession}
            onCloseAllSessions={closeAllSessions}
            onDetachSession={detachSession}
            onDetachAllSessions={detachAllSessions}
            playbookTabs={playbookTabs}
            activePlaybookTabId={activePlaybookTabId}
            onSelectPlaybookTab={selectPlaybookTab}
            onClosePlaybookTab={closePlaybookTab}
            onReorderSessions={(fromIndex, toIndex) => {
              setSessions((prev) => {
                if (
                  fromIndex < 0 || fromIndex >= prev.length ||
                  toIndex < 0 || toIndex >= prev.length ||
                  fromIndex === toIndex
                ) return prev;
                const next = prev.slice();
                const [moved] = next.splice(fromIndex, 1);
                next.splice(toIndex, 0, moved!);
                return next;
              });
            }}
            onReconnectSession={reconnectSession}
            onSessionStatusChange={(id, status, closeInfo) => {
              setSessions((prev) =>
                prev.map((s) => {
                  if (s.id !== id) return s;
                  if (s.status === "reconnecting" && status !== "connected") return s;
                  if (s.status === status) return s;

                  // Notify on real transport-level transitions only.
                  const hostLabel = s.hostName || s.hostId;
                  const protoLabel = s.protocol.toUpperCase();

                  if (status === "connected" &&
                      (s.status === "connecting" || s.status === "reconnecting")) {
                    notify("success", t("connections.connectionEstablished"), {
                      description: t("connections.sessionDescription", {
                        protocol: protoLabel,
                        host: hostLabel,
                      }),
                      source: "connection",
                    });
                  } else if (status === "failed") {
                    const code = closeInfo?.code;
                    const reason = closeInfo?.reason?.trim();
                    // Single source of truth for (code, reason) → i18n keys —
                    // shared with the in-terminal overlay (LiveTerminal), no
                    // second inline regex.
                    const { titleKey, descriptionKey } = sshCloseMessage(code, reason);
                    notify("error", t(titleKey), {
                      description: t(descriptionKey, { host: hostLabel }),
                      source: "connection",
                    });
                  } else if (status === "disconnected" &&
                             (s.status === "connected" || s.status === "reconnecting")) {
                    notify("info", t("connections.sessionEnded"), {
                      description: t("connections.sessionEndedDescription", {
                        protocol: protoLabel,
                        host: hostLabel,
                      }),
                      source: "connection",
                    });
                  }

                  return { ...s, status };
                }),
              );
            }}
            expanded={isMobileSession || panelExpanded}
            onToggleExpand={handleToggleExpand}
            dockPosition={effectiveDockPosition}
            onChangeDock={rememberDockPosition}
            panelSize={panelSize}
            panelMaxSize={panelAvailableSize}
            onPanelResize={(size) => rememberPanelSize(size, effectiveDockPosition, panelAvailableSize)}
          />
        </div>
      )}

      {/* Panel toggle when hidden — orientation matches dock position */}
      {!showPanel && hasOpenTabs && (
        (isMobileSession || effectiveDockPosition === "bottom") ? (
          <div className="flex justify-center shrink-0">
            <Button variant="outline" size="sm" className={cn("surface-chip gap-2 text-sm", isMobileSession ? "h-11" : "h-9")}
              onClick={() => rememberPanelOpen(true)}>
              <Terminal className="h-3 w-3" />
              {t("connections.openTabsCount", { count: openTabsCount })}
            </Button>
          </div>
        ) : (
          <div className="shrink-0 flex items-center justify-center px-1">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="outline"
                  className="surface-chip h-auto w-9 flex-col gap-1 rounded-md px-0 py-2"
                  onClick={() => rememberPanelOpen(true)}
                  aria-label={t("connections.showOpenTabs", { count: openTabsCount })}
                >
                  <Terminal className="h-3.5 w-3.5" aria-hidden="true" />
                  <span aria-hidden="true" className="rounded-full bg-primary/10 px-1.5 py-0.5 text-[11px] font-semibold leading-none tabular-nums text-primary">
                    {openTabsCount}
                  </span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="left" sideOffset={8}>
                {t("connections.showOpenTabs", { count: openTabsCount })}
              </TooltipContent>
            </Tooltip>
          </div>
        )
      )}
      <PlaybookRunsDock
        openTabGroupIds={openPlaybookTabGroupIds}
        onFocusRun={openOrFocusPlaybookTab}
      />
      <PlaybookLaunchDialog
        open={bulkLaunchDialogOpen}
        varsSchema={launchResourceFromQuery(bulkVarsSchema)}
        intent={bulkLaunchIntent}
        onOpenChange={(open) => { if (!open) staticSelectionLaunchMutation.reset(); setBulkLaunchDialogOpen(open); }}
        onConfirm={submitBulkLaunch}
        isSubmitting={staticSelectionLaunchMutation.isPending}
        jobTemplates={bulkLaunchJobs}
        projects={bulkProjectsQuery.data?.items ?? []}
        fixedTargets={bulkLaunchTargets}
        secrets={launchResourceFromQuery(bulkSecretsQuery)}
        onSelectedJobTemplateIdChange={(id) => { staticSelectionLaunchMutation.reset(); setBulkLaunchJobId(id); }}
        loading={bulkJobsQuery.isPending || bulkProjectsQuery.isPending}
        referenceError={bulkJobsQuery.isError || bulkProjectsQuery.isError
          ? t("connections.bulkLaunch.referenceFailed", { defaultValue: "Could not load jobs or projects." })
          : undefined}
        onRetry={() => { void bulkJobsQuery.refetch(); void bulkProjectsQuery.refetch(); }}
        launchError={staticSelectionLaunchMutation.error
          ? t([400, 422].includes(staticSelectionLaunchMutation.error.status)
            ? "connections.bulkLaunch.selectionRejected"
            : staticSelectionLaunchMutation.error.status === 403
              ? "connections.bulkLaunch.forbidden"
              : "connections.bulkLaunch.launchFailed")
          : undefined}
        canLaunch={can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE") && can("EXECUTE_NATIVE", "PLAYBOOK_JOB_TEMPLATE") && !bulkJobsQuery.isError && !bulkProjectsQuery.isError}
      />
      {pendingHostLaunch && <PlaybookLaunchDialog
        open quick intent={{ jobTemplateId: pendingHostLaunch.item.job_template.id }}
        varsSchema={pendingHostLaunch.varsSchema ? { status: "ready", data: pendingHostLaunch.varsSchema } : undefined}
        onOpenChange={(open) => { if (!open) setPendingHostLaunch(null); }}
        onConfirm={(intent) => hostJobLaunch.mutate({ ...pendingHostLaunch, extraVars: intent.extraVars })}
        jobTemplates={[pendingHostLaunch.item.job_template]} projects={bulkProjectsQuery.data?.items ?? []}
        fixedTargets={[toPlaybookLaunchTarget(pendingHostLaunch.host)]}
        secrets={launchResourceFromQuery(bulkSecretsQuery)}
        onSelectedJobTemplateIdChange={() => undefined}
        loading={bulkProjectsQuery.isPending} isSubmitting={hostJobLaunch.isPending}
        referenceError={bulkProjectsQuery.isError ? t("connections.bulkLaunch.referenceFailed", { defaultValue: "Could not load jobs or projects." }) : undefined}
        onRetry={() => { void bulkProjectsQuery.refetch(); void bulkSecretsQuery.refetch(); }}
        launchError={hostJobLaunch.error ? t("connections.bulkLaunch.launchFailed") : undefined}
        canLaunch={can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE") && can("EXECUTE_NATIVE", "PLAYBOOK_JOB_TEMPLATE")}
      />}
      <TreeSettingsDialog
        open={treeSettingsOpen}
        onOpenChange={setTreeSettingsOpen}
        config={treeConfig.config}
        autoOrder={computeAutoOrder(hosts as unknown as Record<string, unknown>[])}
        setMode={treeConfig.setMode}
        addGroup={treeConfig.addGroup}
        removeGroup={treeConfig.removeGroup}
        moveGroup={treeConfig.moveGroup}
        setIncludeUnknown={treeConfig.setIncludeUnknown}
        setQuickAccess={treeConfig.setQuickAccess}
        reset={treeConfig.reset}
      />
      <SessionLaunchDialog controller={sessionLaunch} />
    </div>
  );

  const workspace = renderWorkspace();
  return !hasOpenTabs && viewMode !== "map"
    ? <PageLayout fill className="h-full">{workspace}</PageLayout>
    : workspace;
};

export default ConnectionsWorkspace;
