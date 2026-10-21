import { useRef, useCallback, useEffect, useEffectEvent, useState, type CSSProperties } from "react";
import { useTranslation } from "react-i18next";
import { X, XCircle, ChevronUp, ChevronDown, ChevronLeft, ChevronRight, WifiOff, Loader2, RotateCcw, ExternalLink, PanelBottom, PanelRight, SquareStack, ListChecks } from "lucide-react";
import { beginPanelPointerResize } from "./panelPointerResize";
import SessionPaneChrome, { PaneActions } from "./SessionPaneChrome";
import { paneRectStyle, defaultSessionPaneLayout, normalizeSessionPaneLayout, reconcilePaneLayout, selectPaneSession, sessionPanes, paneGeometry, paneSplitBounds, resizePaneSplit, MIN_SESSION_PANE_SIZE, type SessionPaneLayout } from "./sessionPaneLayout";
import SessionProtocolIcon from "./SessionProtocolIcon";
import { Button } from "@/shared/ui-primitives/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/shared/ui-primitives/tooltip";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import PaneResizer from "@/shared/ui-shared/PaneResizer";
import { clampConnectionsPanelSize, CONNECTIONS_PANEL_MIN_SIZES } from "./panelState";
import LiveSessionViewer from "@/features/connections/components/connections/LiveSessionViewer";
import SessionHealthBar from "@/features/connections/components/connections/SessionHealthBar";
import SessionToolsMenu from "@/features/connections/components/connections/SessionToolsMenu";
import RdpFileBrowserPanel from "@/features/connections/components/connections/RdpFileBrowserPanel";
import SshFileBrowserPanel from "@/features/connections/components/connections/SshFileBrowserPanel";
import type { RdpFilesystem } from "@/features/connections/adapters/ws/rdpFilesystem";
import {
  writeBrowserClipboardText,
  type RemoteClipboardController,
} from "@/features/connections/adapters/ws/remoteClipboard";
import {
  clampSshFileBrowserSize,
  getSshFileBrowserMaxSize,
  loadSshFileBrowserState,
  saveSshFileBrowserState,
  SSH_FILE_BROWSER_MIN_SIZES,
  type FileBrowserPlacement,
  type PersistedSshFileBrowserState,
} from "@/features/connections/components/connections/fileBrowserState";
import { cn } from "@/shared/lib/utils";
import { useSessionViewport } from "@/features/connections/hooks/useSessionViewport";
import { notify } from "@/shared/lib/notifications";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import { isSessionWsConnectable } from "@/features/connections/adapters/ws/urls";
import PlaybookTabViewer from "@/features/playbooks/PlaybookTabViewer";
import { playbookStatusDotClassName, type PlaybookTab } from "@/features/playbooks/playbookTabs";
import { runStatusLabelKey } from "@/features/playbooks/runPhase";
import { playbookRunGroupHref } from "@/features/playbooks/playbookNavigation";

export type DockPosition = "bottom" | "right";

const FILE_BROWSER_TERMINAL_RESERVED_SIZE: Record<FileBrowserPlacement, number> = {
  bottom: 160,
  right: 320,
};

const FILE_BROWSER_KEYBOARD_STEP = 24;
const FILE_BROWSER_KEYBOARD_LARGE_STEP = 96;

interface FileBrowserSplitBounds {
  width: number;
  height: number;
}

function fileBrowserAvailableSize(
  placement: FileBrowserPlacement,
  bounds: FileBrowserSplitBounds | null,
): number | undefined {
  if (!bounds) return undefined;
  const axisSize = placement === "right" ? bounds.width : bounds.height;
  if (!Number.isFinite(axisSize) || axisSize <= 0) return undefined;
  return axisSize - FILE_BROWSER_TERMINAL_RESERVED_SIZE[placement];
}

/**
 * Single-line label that ellipsis-truncates at rest and, only when the text
 * actually overflows, scrolls the full value as a marquee while the parent
 * `.connection-tab` is hovered. Overflow is measured in JS because CSS alone
 * can't gate the animation on "is this truncated"; the `.connection-tab:hover`
 * rule + keyframes live in index.css. Honors prefers-reduced-motion there.
 */
function MarqueeText({ text, className }: { text: string; className?: string }) {
  const innerRef = useRef<HTMLSpanElement>(null);
  const [overflowPx, setOverflowPx] = useState(0);

  useEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const measure = () => setOverflowPx(Math.max(0, el.scrollWidth - el.clientWidth));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [text]);

  const overflowing = overflowPx > 0;
  // Constant scroll speed (~45px/s), clamped so short and very long names both
  // stay readable rather than scaling duration linearly without bound.
  const durationS = Math.min(12, Math.max(2.5, overflowPx / 45));

  return (
    <span className={cn("relative block min-w-0 flex-1 overflow-hidden", className)}>
      <span
        ref={innerRef}
        className="connection-tab-name font-medium"
        data-overflowing={overflowing ? "true" : "false"}
        style={overflowing
          ? ({ "--marquee-shift": `-${overflowPx}px`, "--marquee-duration": `${durationS}s` } as CSSProperties)
          : undefined}
      >
        {text}
      </span>
    </span>
  );
}

interface Props {
  paneLayout?: SessionPaneLayout | null;
  onPaneLayoutChange?: (layout: SessionPaneLayout) => void;
  mobile?: boolean;
  visible?: boolean;
  sessions: LiveSession[];
  activeSessionId: string | null;
  onSelectSession: (id: string | null) => void;
  onCloseSession: (id: string) => void;
  onCloseAllSessions: () => void;
  onDetachSession: (id: string) => void;
  /** Playbook run tabs — rendered after session tabs in the same strip. */
  playbookTabs: PlaybookTab[];
  activePlaybookTabId: string | null;
  onSelectPlaybookTab: (id: string) => void;
  onClosePlaybookTab: (id: string) => void;
  /**
   * Open every session in a single new window, preserving the current tab
   * order. Optional: the in-panel close-all button is always available, but
   * detach-all only appears when this callback is wired.
   */
  onDetachAllSessions?: () => void;
  /** Reorder the in-panel session tabs by dragging. Called on a successful drop. */
  onReorderSessions?: (fromIndex: number, toIndex: number) => void;
  onReconnectSession: (session: LiveSession) => void;
  /**
   * Status transition for a session. `closeInfo` carries the WebSocket close
   * frame on disconnect/failure so callers can classify the error (network,
   * auth, server) and surface a specific user-facing notification.
   */
  onSessionStatusChange: (
    id: string,
    status: LiveSession["status"],
    closeInfo?: { code?: number; reason?: string },
  ) => void;
  expanded: boolean;
  onToggleExpand: () => void;
  dockPosition: DockPosition;
  onChangeDock: (pos: DockPosition) => void;
  panelSize: number;
  panelMaxSize?: number;
  onPanelResize: (size: number) => void;
}

function desktopPanelStyleFor(
  isBottom: boolean,
  expanded: boolean,
  size: number,
): CSSProperties {
  if (isBottom) return { height: size };
  if (expanded) return { width: size, flexShrink: 0, order: 1 };
  return { width: 36, order: 1 };
}

const ConnectionsPanel = ({
  paneLayout: persistedPaneLayout, onPaneLayoutChange,
  mobile = false, visible = true,
  sessions, activeSessionId, onSelectSession: notifySessionSelection, onCloseSession, onDetachSession,
  onDetachAllSessions, onReorderSessions,
  onCloseAllSessions, onReconnectSession, onSessionStatusChange,
  playbookTabs, activePlaybookTabId, onSelectPlaybookTab, onClosePlaybookTab,
  expanded, onToggleExpand, dockPosition, onChangeDock, panelSize, panelMaxSize, onPanelResize
}: Props) => {
  const { t } = useTranslation();
  const viewportRef = useSessionViewport(mobile);
  const [gridSize, setGridSize] = useState<{ width: number; height: number } | null>(null);
  const gridObserver = useRef<ResizeObserver | null>(null);
  const gridRef = useCallback((node: HTMLDivElement | null) => {
    gridObserver.current?.disconnect();
    if (!node) return;
    const measure = () => {
      const rect = node.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) setGridSize({ width: rect.width, height: rect.height });
    };
    gridObserver.current = new ResizeObserver(measure);
    gridObserver.current.observe(node);
    measure();
  }, []);
  // HTML5 drag-reorder state for the tab strip. `dragIndex` is the index of the
  // tab currently being dragged; `dragOverIndex` is the slot the cursor is over
  // (drop target). Both null when no drag is in progress.
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null);
  const activePlaybookTab = playbookTabs.find((tab) => tab.id === activePlaybookTabId) ?? null;
  const persistedLayout = normalizeSessionPaneLayout(persistedPaneLayout);
  const persistedSignature = JSON.stringify(persistedLayout);
  const [paneState, setPaneState] = useState(() => {
    const initialLayout = persistedLayout ?? defaultSessionPaneLayout(activeSessionId);
    return {
      layout: initialLayout,
      selectedId: activeSessionId,
      persistedSignature, sessionIds: sessions.map(s => s.id).join(","),
    };
  });
  let layout = reconcilePaneLayout(paneState.layout, sessions.map(session => session.id));
  if (persistedSignature !== paneState.persistedSignature) {
    layout = reconcilePaneLayout(persistedLayout ?? layout, sessions.map(session => session.id));
  }
  if (activeSessionId && activeSessionId !== paneState.selectedId) {
    layout = selectPaneSession(layout, activeSessionId);
  }
  const sessionIds = sessions.map(s => s.id).join(",");
  if (activeSessionId !== paneState.selectedId || persistedSignature !== paneState.persistedSignature || sessionIds !== paneState.sessionIds) {
    setPaneState({ layout, selectedId: activeSessionId, persistedSignature, sessionIds });
  }
  const panes = sessionPanes(layout.root);
  const groupedSessionIds = new Set(panes.flatMap(pane => pane.sessionId ? [pane.sessionId] : []));
  const groupedSessions = panes.length > 1 ? sessions.filter(session => groupedSessionIds.has(session.id)) : [];
  const layoutRef = useRef(layout);
  useEffect(() => { layoutRef.current = layout; });
  // Group-member navigation changes focus/maximize, not assignments. Persist
  // that change too, while standalone navigation remains layout-neutral.
  const reconciliationInput = useRef({ sessionIds, activeSessionId });
  const persistReconciledLayout = useEffectEvent((sessionsChanged: boolean) => {
    const next = layoutRef.current;
    const selectingGroupMember = activeSessionId !== null && next.root.kind === "split" && sessionPanes(next.root).some(pane => pane.sessionId === activeSessionId);
    if (sessionsChanged || (selectingGroupMember && JSON.stringify(next) !== persistedSignature)) onPaneLayoutChange?.(next);
  });
  useEffect(() => {
    const previous = reconciliationInput.current;
    if (previous.sessionIds === sessionIds && previous.activeSessionId === activeSessionId) return;
    reconciliationInput.current = { sessionIds, activeSessionId };
    persistReconciledLayout(previous.sessionIds !== sessionIds);
  }, [sessionIds, activeSessionId]);
  const showingGroup = panes.length > 1 && (activeSessionId === null || groupedSessionIds.has(activeSessionId));
  const focusedSessionId = panes.find(pane => pane.id === layout.focusedPaneId)?.sessionId;
  const activeSession = activePlaybookTab ? undefined : sessions.find(s => s.id === (showingGroup ? focusedSessionId : activeSessionId));
  const changePaneLayout = (next: SessionPaneLayout, persist = true) => {
    layoutRef.current = next;
    setPaneState({ layout: next, selectedId: activeSessionId, persistedSignature, sessionIds });
    if (persist) onPaneLayoutChange?.(next);
    const selected = sessionPanes(next.root).find(pane => pane.id === next.focusedPaneId)?.sessionId;
    if (selected !== activeSessionId) notifySessionSelection(selected ?? null);
  };
  const onSelectSession = (sessionId: string) => notifySessionSelection(sessionId);
  const focusPane = (paneId: string) => changePaneLayout({ ...layout, focusedPaneId: paneId });
  const geometry = paneGeometry(layout.root, undefined, mobile);
  const singleOccupiedPane = !showingGroup && Boolean(activeSession);
  const standaloneLayout = defaultSessionPaneLayout(activeSessionId);
  const paneHeaderOffset = singleOccupiedPane ? "0px" : "var(--session-pane-header-height)";
  const visiblePaneCount = layout.maximizedPaneId ? 1 : panes.length;
  const minimumGridHeight = mobile && showingGroup
    ? `calc(${visiblePaneCount} * (${MIN_SESSION_PANE_SIZE.height}px + ${paneHeaderOffset}))`
    : undefined;
  const sessionRect = (sessionId: string): CSSProperties => {
    if (!showingGroup) return paneRectStyle({ x: 0, y: 0, width: 100, height: 100 });
    const pane = geometry.find(item => item.node.kind === "pane" && item.node.sessionId === sessionId);
    const rect = layout.maximizedPaneId ? { x: 0, y: 0, width: 100, height: 100 } : pane?.rect;
    if (!rect) return {};
    return { ...paneRectStyle(rect), top: `calc(${rect.y}% + ${paneHeaderOffset})`, height: `calc(${rect.height}% - ${paneHeaderOffset})` };
  };
  const renderSessionHealth = (session: LiveSession, compact = false) => session.protocol === "cctv" ? null : (
    <SessionHealthBar key={session.id} sessionId={session.id}
      enabled={visible && expanded && !activePlaybookTab}
      viewerStatus={session.status}
      className={mobile ? "size-11 min-h-11 min-w-11" : compact ? "size-7" : undefined}
      onConnectionLost={session.protocol === "rdp" || session.protocol === "vnc"
        ? () => onSessionStatusChange(session.id, "failed") : undefined} />
  );
  const activeSessionCanConnect = activeSession
    ? isSessionWsConnectable(activeSession.websocketUrl, activeSession.token)
    : false;
  const resizeCleanup = useRef<(() => void) | null>(null);
  const [resizeCursor, setResizeCursor] = useState<"ew-resize" | "ns-resize" | null>(null);
  const [fileBrowserSessionId, setFileBrowserSessionId] = useState<string | null>(null);
  // RDP redirected drives, keyed by session id. Set when guacd exposes the
  // drive (client `filesystem` instruction), cleared when the viewer unmounts.
  const [rdpFilesystems, setRdpFilesystems] = useState<Record<string, RdpFilesystem>>({});
  const [clipboardControllers, setClipboardControllers] =
    useState<Record<string, RemoteClipboardController>>({});
  const [fileBrowserState, setFileBrowserState] = useState(loadSshFileBrowserState);
  const [fileBrowserSplitBounds, setFileBrowserSplitBounds] =
    useState<FileBrowserSplitBounds | null>(null);
  const fileBrowserStateRef = useRef(fileBrowserState);
  const fileBrowserSplitObserverRef = useRef<ResizeObserver | null>(null);
  // Connection tab strip: vertical mouse wheel scrolls the tabs horizontally
  // (native non-passive listener — React's synthetic onWheel is passive and
  // can't preventDefault), and we track overflow so the paging arrows show only
  // when there is somewhere to scroll.
  const tabBarNode = useRef<HTMLDivElement | null>(null);
  const tabBarCleanup = useRef<(() => void) | null>(null);
  const [tabScroll, setTabScroll] = useState({ left: false, right: false });
  const updateTabScroll = useCallback(() => {
    const node = tabBarNode.current;
    if (!node) return;
    const max = node.scrollWidth - node.clientWidth;
    setTabScroll((current) => {
      const next = { left: node.scrollLeft > 1, right: node.scrollLeft < max - 1 };
      return current.left === next.left && current.right === next.right ? current : next;
    });
  }, []);
  const tabBarRef = useCallback((node: HTMLDivElement | null) => {
    tabBarCleanup.current?.();
    tabBarCleanup.current = null;
    tabBarNode.current = node;
    if (!node) return;
    const onWheel = (event: WheelEvent) => {
      if (node.scrollWidth <= node.clientWidth) return;
      // Leave horizontal-intent (trackpad) scrolling to the browser.
      if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
      event.preventDefault();
      node.scrollLeft += event.deltaY;
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    node.addEventListener("scroll", updateTabScroll, { passive: true });
    const observer =
      typeof ResizeObserver !== "undefined" ? new ResizeObserver(updateTabScroll) : null;
    observer?.observe(node);
    updateTabScroll();
    tabBarCleanup.current = () => {
      node.removeEventListener("wheel", onWheel);
      node.removeEventListener("scroll", updateTabScroll);
      observer?.disconnect();
    };
  }, [updateTabScroll]);
  const scrollTabs = useCallback((direction: -1 | 1) => {
    const node = tabBarNode.current;
    if (!node) return;
    node.scrollBy({ left: direction * Math.max(120, node.clientWidth * 0.7), behavior: "smooth" });
  }, []);
  const handleRdpFilesystem = useCallback((sessionId: string, fs: RdpFilesystem | null) => {
    setRdpFilesystems((current) => {
      if (fs) return { ...current, [sessionId]: fs };
      if (!(sessionId in current)) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }, []);
  const clearClipboardController = useCallback((sessionId: string) => {
    setClipboardControllers((current) => {
      if (!(sessionId in current)) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }, [setClipboardControllers]);
  const handleClipboardController = useCallback((
    sessionId: string,
    controller: RemoteClipboardController | null,
  ) => {
    setClipboardControllers((current) => {
      if (controller) return { ...current, [sessionId]: controller };
      if (!(sessionId in current)) return current;
      const next = { ...current };
      delete next[sessionId];
      return next;
    });
  }, [setClipboardControllers]);
  const handleRemoteClipboardText = (session: LiveSession, text: string) => {
    if (!visible || !expanded || activePlaybookTab || session.id !== focusedSessionId) return;
    void writeBrowserClipboardText(text).then(
      () => {
        notify("success", t("connections.clipboard.received"), {
          description: t("connections.clipboard.receivedDescription", {
            protocol: session.protocol.toUpperCase(),
            host: session.hostName,
          }),
          source: "connections",
        });
      },
      () => {
        notify("warning", t("connections.clipboard.receiveFailed"), {
          description: t("connections.clipboard.receiveFailedDescription"),
          source: "connections",
        });
      },
    );
  };
  const handleSendClipboard = useCallback((session: LiveSession) => {
    const controller = clipboardControllers[session.id];
    if (!controller) return;
    if (!navigator.clipboard?.readText) {
      notify("warning", t("connections.clipboard.sendUnavailable"), {
        description: t("connections.clipboard.sendUnavailableDescription"),
        source: "connections",
      });
      return;
    }
    void navigator.clipboard.readText().then(
      (text) => {
        if (!text) {
          notify("info", t("connections.clipboard.empty"), { source: "connections" });
          return;
        }
        try {
          controller.sendText(text);
          notify("success", t("connections.clipboard.sent"), {
            description: t("connections.clipboard.sentDescription", {
              protocol: session.protocol.toUpperCase(),
              host: session.hostName,
            }),
            source: "connections",
          });
        } catch {
          notify("error", t("connections.clipboard.sendFailed"), {
            description: t("connections.clipboard.sendFailedDescription"),
            source: "connections",
          });
        }
      },
      () => {
        notify("warning", t("connections.clipboard.readFailed"), {
          description: t("connections.clipboard.readFailedDescription"),
          source: "connections",
        });
      },
    );
  }, [clipboardControllers, t]);
  const handleCloseSession = useCallback((sessionId: string) => {
    clearClipboardController(sessionId);
    onCloseSession(sessionId);
  }, [clearClipboardController, onCloseSession]);
  const handleCloseAllSessions = useCallback(() => {
    setClipboardControllers((current) => (Object.keys(current).length > 0 ? {} : current));
    onCloseAllSessions();
  }, [onCloseAllSessions, setClipboardControllers]);
  const handleReconnectSession = useCallback((session: LiveSession) => {
    clearClipboardController(session.id);
    onReconnectSession(session);
  }, [clearClipboardController, onReconnectSession]);

  const sessionSupportsFileBrowser = useCallback(
    (session: LiveSession): boolean =>
      session.status === "connected" &&
      (session.protocol === "ssh" ||
        (session.protocol === "rdp" && !!rdpFilesystems[session.id])),
    [rdpFilesystems],
  );

  const activeSessionFileBrowserOpen =
    !!activeSession &&
    sessionSupportsFileBrowser(activeSession) &&
    fileBrowserSessionId === activeSession.id;
  const activeVncSshFileSessions =
    activeSession?.protocol === "vnc"
      ? sessions.filter(
          (session) =>
            session.hostId === activeSession.hostId &&
            session.protocol === "ssh" &&
            session.status === "connected",
        )
      : [];
  const activeVncSshFileSession =
    activeSession?.username
      ? activeVncSshFileSessions.find((session) => session.username === activeSession.username) ??
        activeVncSshFileSessions[0]
      : activeVncSshFileSessions[0];

  useEffect(() => {
    fileBrowserStateRef.current = fileBrowserState;
  }, [fileBrowserState]);

  // Adding/removing a tab changes scrollWidth without resizing the container,
  // so recompute the arrow/overflow state when the session set or dock changes.
  useEffect(() => {
    updateTabScroll();
  }, [sessions.length, playbookTabs.length, expanded, dockPosition, updateTabScroll]);

  const setFileBrowserSplitElement = useCallback((node: HTMLDivElement | null) => {
    fileBrowserSplitObserverRef.current?.disconnect();
    fileBrowserSplitObserverRef.current = null;

    if (!node || typeof ResizeObserver === "undefined") {
      setFileBrowserSplitBounds(null);
      return;
    }

    const updateBounds = () => {
      const rect = node.getBoundingClientRect();
      setFileBrowserSplitBounds((current) => {
        const next = {
          width: Math.round(rect.width),
          height: Math.round(rect.height),
        };
        return current?.width === next.width && current.height === next.height
          ? current
          : next;
      });
    };

    updateBounds();
    const observer = new ResizeObserver(updateBounds);
    observer.observe(node);
    fileBrowserSplitObserverRef.current = observer;
  }, [setFileBrowserSplitBounds]);

  // Only render an icon for in-progress states (connecting / reconnecting). For
  // steady states the colored left accent on the tab already conveys status,
  // so an extra wifi/wifi-off icon would just be visual noise next to the
  // protocol icon.
  const statusIcon = (status: LiveSession["status"]) => {
    switch (status) {
      case "connecting": return <Loader2 className="h-3 w-3 animate-spin text-status-warning" />;
      case "reconnecting": return <RotateCcw className="h-3 w-3 animate-spin text-status-warning" />;
      default: return null;
    }
  };

  // Status-colored left accent so connection state reads at a glance, even
  // before the icon is parsed.
  const statusBorderClass = (status: LiveSession["status"]) => {
    switch (status) {
      case "connected": return "border-l-status-positive";
      case "connecting":
      case "reconnecting": return "border-l-status-warning";
      case "failed": return "border-l-status-negative";
      default: return "border-l-border/40";
    }
  };

  const statusLabel = (status: LiveSession["status"]) =>
    t(`connections.sessionStatus.${status}`);

  useEffect(() => () => {
    resizeCleanup.current?.();
    fileBrowserSplitObserverRef.current?.disconnect();
    document.body.style.cursor = "";
    document.body.style.userSelect = "";
  }, []);

  const updateFileBrowserState = useCallback((
    updater: (state: PersistedSshFileBrowserState) => PersistedSshFileBrowserState,
    persist = true,
  ) => {
    setFileBrowserState((current) => {
      const next = updater(current);
      fileBrowserStateRef.current = next;
      if (persist) saveSshFileBrowserState(next);
      return next;
    });
  }, [setFileBrowserState]);

  const startPointerResize = useCallback((event: React.PointerEvent<HTMLElement>, axis: "x" | "y", change: (delta: number) => void, finish?: () => void) => {
    if (event.button !== 0) return;
    resizeCleanup.current?.();
    setResizeCursor(axis === "x" ? "ew-resize" : "ns-resize");
    resizeCleanup.current = beginPanelPointerResize(event, axis, change, () => {
      setResizeCursor(null);
      resizeCleanup.current = null;
      finish?.();
    });
  }, []);

  const onFileBrowserResizePointerDown = useCallback((event: React.PointerEvent<HTMLDivElement>, placement: FileBrowserPlacement) => {
    const splitRoot = event.currentTarget.closest("[data-ssh-file-browser-split]");
    const rect = splitRoot instanceof HTMLElement ? splitRoot.getBoundingClientRect() : null;
    const available = rect ? fileBrowserAvailableSize(placement, rect) : undefined;
    const size = fileBrowserStateRef.current.sizes[placement];
    startPointerResize(event, placement === "bottom" ? "y" : "x", delta => {
      updateFileBrowserState(current => ({ ...current, sizes: { ...current.sizes, [placement]: clampSshFileBrowserSize(placement, size - delta, available) } }), false);
    }, () => saveSshFileBrowserState(fileBrowserStateRef.current));
  }, [updateFileBrowserState, startPointerResize]);

  const onFileBrowserResizeKeyDown = useCallback((
    event: React.KeyboardEvent<HTMLDivElement>,
    placement: FileBrowserPlacement,
    currentSize: number,
    availableSize?: number,
  ) => {
    const step = event.shiftKey ? FILE_BROWSER_KEYBOARD_LARGE_STEP : FILE_BROWSER_KEYBOARD_STEP;
    let nextSize: number | null = null;
    if (event.key === "Home") {
      nextSize = SSH_FILE_BROWSER_MIN_SIZES[placement];
    } else if (event.key === "End") {
      nextSize = getSshFileBrowserMaxSize(placement, availableSize);
    } else if (placement === "right" && event.key === "ArrowLeft") {
      nextSize = currentSize + step;
    } else if (placement === "right" && event.key === "ArrowRight") {
      nextSize = currentSize - step;
    } else if (placement === "bottom" && event.key === "ArrowUp") {
      nextSize = currentSize + step;
    } else if (placement === "bottom" && event.key === "ArrowDown") {
      nextSize = currentSize - step;
    }

    if (nextSize === null) return;
    event.preventDefault();
    updateFileBrowserState((current) => ({
      ...current,
      sizes: {
        ...current.sizes,
        [placement]: clampSshFileBrowserSize(placement, nextSize, availableSize),
      },
    }));
  }, [updateFileBrowserState]);

  const isBottom = mobile || dockPosition === "bottom";
  const effectiveFileBrowserPlacement: FileBrowserPlacement =
    (typeof window !== "undefined" && window.innerWidth < 768) || (!isBottom && panelSize < 680)
      ? "bottom"
      : fileBrowserState.placement;
  // When docked on the right and collapsed, the panel is a narrow 36px strip.
  // A horizontal tab bar clips its labels/controls there, so render a vertical
  // rail instead: stacked session icons + the expand / dock / close controls.
  const collapsedRail = !expanded && !isBottom;
  const desktopPanelSize = expanded ? panelSize : 36;
  const desktopPanelStyle = desktopPanelStyleFor(isBottom, expanded, desktopPanelSize);
  const panelStyle = mobile
    ? { height: "var(--session-viewport-height, 100dvh)", top: "var(--session-viewport-top, 0px)" }
    : desktopPanelStyle;

  return (
    <div
      ref={viewportRef}
      data-testid="session-workspace"
      className={cn(
        "flex min-w-0 max-w-full overflow-hidden shrink-0",
        mobile ? "fixed inset-x-0 z-50 w-full flex-col bg-background pt-[env(safe-area-inset-top)]" : "animate-slide-up",
        isBottom ? "w-full flex-col" : "flex-row",
        !visible && "hidden"
      )}
      style={panelStyle}
    >
      {/* Resize handle */}
      {expanded && !mobile && (
        <PaneResizer orientation={isBottom ? "vertical" : "horizontal"} reverse
          size={panelSize} min={CONNECTIONS_PANEL_MIN_SIZES[dockPosition]}
          max={clampConnectionsPanelSize(dockPosition, Number.MAX_SAFE_INTEGER, panelMaxSize)}
          onResize={onPanelResize} label={t("connections.workspace.resizeSessions")} />
      )}

      <SurfaceCard className={cn("flex flex-col flex-1 overflow-hidden min-h-0 min-w-0 rounded-none", !isBottom && "rounded-l-lg")}>
        {/* Collapsed right-dock rail: vertical strip so labels/controls stay visible */}
        {collapsedRail ? (
          <div className="flex flex-1 flex-col items-center w-full py-1.5 gap-1 min-h-0">
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={onToggleExpand}
              aria-label={t("connections.expand")}
              title={t("connections.expand")}
            >
              <ChevronLeft className="h-4 w-4" />
            </Button>
            <div className="w-6 border-t border-border/30 my-0.5" />
            <div role="tablist" className="flex flex-col items-center gap-1 w-full overflow-y-auto min-h-0">
              {sessions.map(session => (
                <button
                  key={session.id}
                  type="button"
                  role="tab"
                  aria-selected={session.id === activeSessionId}
                  onClick={() => { onSelectSession(session.id); onToggleExpand(); }}
                  title={`${session.protocol.toUpperCase()} · ${session.hostName} · ${statusLabel(session.status)}`}
                  className={cn(
                    "flex w-7 flex-col items-center gap-1 rounded-md py-1.5 transition-colors shrink-0",
                    session.id === activeSessionId
                      ? "bg-accent text-foreground"
                      : "text-muted-foreground hover:text-foreground hover:bg-accent"
                  )}
                >
                  {statusIcon(session.status)}
                  <SessionProtocolIcon protocol={session.protocol} compact />
                </button>
              ))}
              {playbookTabs.map(tab => (
                <button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={tab.id === activePlaybookTabId}
                  onClick={() => { onSelectPlaybookTab(tab.id); onToggleExpand(); }}
                  title={`${tab.playbookName} · ${tab.hostSummary} · ${t(runStatusLabelKey(tab.phase))}`}
                  className={cn(
                    "relative flex w-7 flex-col items-center gap-1 rounded-md py-1.5 transition-colors shrink-0",
                    tab.id === activePlaybookTabId
                      ? "bg-accent text-foreground"
                      : "text-muted-foreground hover:text-foreground hover:bg-accent"
                  )}
                >
                  <ListChecks className="h-3.5 w-3.5" />
                  <span
                    aria-hidden
                    className={cn("absolute right-1 top-1 h-1.5 w-1.5 rounded-full", playbookStatusDotClassName(tab.phase))}
                  />
                </button>
              ))}
            </div>
            <div className="mt-auto flex flex-col items-center gap-1 w-full">
              <div className="w-6 border-t border-border/30 my-0.5" />
              <Button
                variant="ghost" size="icon-sm"
                onClick={() => onChangeDock("bottom")}
                aria-label={t("connections.dockBottom")}
                title={t("connections.dockBottom")}
              >
                <PanelBottom className="h-4 w-4" />
              </Button>
              {sessions.length > 0 && (
                <Button
                  variant="ghost"
                  size="icon-sm"
                  className="text-muted-foreground hover:text-destructive"
                  onClick={handleCloseAllSessions}
                  aria-label={t("connections.closeAllSessions")}
                  title={t("connections.closeAllSessions")}
                >
                  <XCircle className="h-4 w-4" />
                </Button>
              )}
            </div>
          </div>
        ) : (
        /* Tab bar */
        <div className={cn("flex min-w-0 items-center border-b border-border/30 shrink-0", mobile ? "h-11" : "h-9")}>
          {mobile && (
            <Button variant="ghost" size="icon" className="size-11 shrink-0"
              onClick={onToggleExpand} aria-label={t("connections.detachedGroupBack")}>
              <ChevronLeft className="size-5" aria-hidden="true" />
            </Button>
          )}
          <div className="relative flex items-center flex-1 min-w-0">
            {tabScroll.left && (
              <button
                type="button"
                onClick={() => scrollTabs(-1)}
                aria-label={t("connections.scrollTabsLeft")}
                className="absolute left-0 top-0 z-10 inline-flex h-full min-w-6 items-center justify-center border-r border-border bg-card text-muted-foreground hover:text-foreground"
              >
                <ChevronLeft className="h-4 w-4" />
              </button>
            )}
            <div ref={tabBarRef} className="flex items-center w-full overflow-x-auto tab-strip-scroll gap-px px-1 min-w-0" role="tablist">
            {panes.length > 1 && <button type="button" role="tab" aria-selected={!activePlaybookTabId && showingGroup}
              title={groupedSessions.map(session => `${session.protocol.toUpperCase()} · ${session.hostName}`).join("\n")}
              className="connection-tab flex min-w-0 shrink-0 items-center gap-1.5 rounded-t-md bg-accent px-3 py-1.5 text-sm font-medium"
              onClick={() => notifySessionSelection(focusedSessionId ?? null)}>
              <SquareStack className="size-4 shrink-0" aria-hidden="true" />
              {t("connections.panes.group")} · {groupedSessions.length}
            </button>}
            <TooltipProvider delayDuration={300}>
            {sessions.map((session, index) => groupedSessions.some(member => member.id === session.id) ? null : (
              <Tooltip key={session.id}>
                <TooltipTrigger asChild>
                  <div
                    draggable={onReorderSessions !== undefined}
                    onDragStart={(e) => {
                      if (!onReorderSessions) return;
                      setDragIndex(index);
                      e.dataTransfer.effectAllowed = "move";
                      // Required by Firefox; the value is ignored.
                      e.dataTransfer.setData("text/plain", session.id);
                    }}
                    onDragOver={(e) => {
                      if (dragIndex === null) return;
                      e.preventDefault();
                      e.dataTransfer.dropEffect = "move";
                      if (dragOverIndex !== index) setDragOverIndex(index);
                    }}
                    onDragLeave={() => {
                      if (dragOverIndex === index) setDragOverIndex(null);
                    }}
                    onDrop={(e) => {
                      e.preventDefault();
                      if (onReorderSessions && dragIndex !== null && dragIndex !== index) {
                        onReorderSessions(dragIndex, index);
                      }
                      setDragIndex(null);
                      setDragOverIndex(null);
                    }}
                    onDragEnd={() => {
                      setDragIndex(null);
                      setDragOverIndex(null);
                    }}
                    className={cn(
                      "connection-tab grid w-auto min-w-[8.5rem] max-w-[15rem] shrink-0 grid-cols-[minmax(0,1fr)_auto_auto] items-center rounded-t-md border-l-2 text-sm transition-colors whitespace-nowrap group/tab",
                      statusBorderClass(session.status),
                      session.id === activeSessionId
                        ? "bg-accent text-foreground font-medium"
                        : "text-muted-foreground hover:text-foreground hover:bg-accent",
                      dragIndex === index && "opacity-50",
                      dragOverIndex === index && dragIndex !== null && dragIndex !== index && "ring-1 ring-primary",
                      onReorderSessions !== undefined && "cursor-grab active:cursor-grabbing",
                    )}
                  >
                    <button
                      type="button"
                      role="tab"
                      aria-selected={session.id === activeSessionId}
                      onClick={() => onSelectSession(session.id)}
                      className={cn("flex min-w-0 items-center gap-1.5 px-2 py-1.5", mobile && "min-h-11")}
                    >
                      <span className="shrink-0">{statusIcon(session.status)}</span>
                      <SessionProtocolIcon protocol={session.protocol} />
                      <MarqueeText text={session.hostName} />
                    </button>
                    <button
                      type="button"
                      onClick={() => onDetachSession(session.id)}
                      className={cn("inline-flex size-9 shrink-0 items-center justify-center rounded-sm hover:bg-accent hover:text-primary transition-colors", mobile && "hidden")}
                      aria-label={t("connections.detachSession")}
                    >
                      <ExternalLink className="h-3 w-3" />
                    </button>
                    <button
                      type="button"
                      onClick={() => handleCloseSession(session.id)}
                      className={cn("inline-flex size-9 shrink-0 items-center justify-center rounded-sm hover:bg-accent hover:text-destructive transition-colors", mobile && "size-11")}
                      aria-label={t("connections.closeSession")}
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                </TooltipTrigger>
                <TooltipContent side="bottom" className="text-xs max-w-xs">
                  <div className="space-y-0.5">
                    <div className="font-medium">{session.hostName}</div>
                    <div className="font-mono text-xs uppercase text-muted-foreground">
                      {session.protocol} · {statusLabel(session.status)}
                    </div>
                    {session.hostRole && (
                      <div><span className="text-muted-foreground">{t("connections.columns.role")}:</span> {session.hostRole}</div>
                    )}
                    {session.hostSite && (
                      <div><span className="text-muted-foreground">{t("connections.columns.site")}:</span> {session.hostSite}</div>
                    )}
                    {session.hostRegion && (
                      <div><span className="text-muted-foreground">{t("connections.filtersPanel.region")}:</span> {session.hostRegion}</div>
                    )}
                    {session.hostTenant && (
                      <div><span className="text-muted-foreground">{t("connections.columns.tenant")}:</span> {session.hostTenant}</div>
                    )}
                    {session.hostCluster && (
                      <div><span className="text-muted-foreground">{t("connections.filtersPanel.cluster")}:</span> {session.hostCluster}</div>
                    )}
                  </div>
                </TooltipContent>
              </Tooltip>
            ))}
            </TooltipProvider>
            {sessions.length > 0 && playbookTabs.length > 0 && (
              <div
                aria-hidden
                data-testid="tab-kind-divider"
                className="mx-1 h-5 w-px shrink-0 self-center bg-border/40"
              />
            )}
            <TooltipProvider delayDuration={300}>
            {playbookTabs.map((tab) => (
              <Tooltip key={tab.id}>
                <TooltipTrigger asChild>
                  <div
                    className={cn(
                      "connection-tab grid w-auto min-w-[8.5rem] max-w-[15rem] shrink-0 grid-cols-[minmax(0,1fr)_auto_auto] items-center rounded-t-md border-l-2 text-sm transition-colors whitespace-nowrap group/tab",
                      "border-l-transparent",
                      tab.id === activePlaybookTabId
                        ? "bg-accent text-foreground font-medium"
                        : "text-muted-foreground hover:text-foreground hover:bg-accent",
                    )}
                  >
                    <button
                      type="button"
                      role="tab"
                      aria-selected={tab.id === activePlaybookTabId}
                      onClick={() => onSelectPlaybookTab(tab.id)}
                      className="flex min-w-0 items-center gap-1.5 px-2 py-1.5"
                    >
                      <span
                        aria-hidden
                        className={cn("h-1.5 w-1.5 shrink-0 rounded-full", playbookStatusDotClassName(tab.phase))}
                      />
                      <ListChecks className="h-3 w-3 shrink-0" />
                      <MarqueeText text={`${tab.playbookName} · ${tab.hostSummary}`} />
                    </button>
                    <button
                      type="button"
                      onClick={() => window.open(playbookRunGroupHref(tab.groupId, tab.playbookName), "_blank")}
                      className="inline-flex size-9 shrink-0 items-center justify-center rounded-sm hover:bg-accent hover:text-primary transition-colors"
                      aria-label={t("playbookTab.detachTab")}
                      title={t("playbookTab.detachTab")}
                    >
                      <ExternalLink className="h-3 w-3" />
                    </button>
                    <button
                      type="button"
                      onClick={() => onClosePlaybookTab(tab.id)}
                      className="inline-flex size-9 shrink-0 items-center justify-center rounded-sm hover:bg-accent hover:text-destructive transition-colors"
                      aria-label={t("playbookTab.closeTab")}
                      title={t("playbookTab.closeTab")}
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                </TooltipTrigger>
                <TooltipContent side="bottom" className="text-xs max-w-xs">
                  <div className="space-y-0.5">
                    <div className="font-medium">{tab.playbookName}</div>
                    <div className="text-xs text-muted-foreground">
                      {tab.hostSummary} · {t(runStatusLabelKey(tab.phase))}
                    </div>
                  </div>
                </TooltipContent>
              </Tooltip>
            ))}
            </TooltipProvider>
            </div>
            {tabScroll.right && (
              <button
                type="button"
                onClick={() => scrollTabs(1)}
                aria-label={t("connections.scrollTabsRight")}
                className="absolute right-0 top-0 z-10 inline-flex h-full min-w-6 items-center justify-center border-l border-border bg-card text-muted-foreground hover:text-foreground"
              >
                <ChevronRight className="h-4 w-4" />
              </button>
            )}
          </div>
          <div className="flex items-center gap-0.5 px-1.5 shrink-0 border-l border-border/20">
            {singleOccupiedPane && activeSession && <>
              {renderSessionHealth(activeSession)}
              <PaneActions node={sessionPanes(standaloneLayout.root)[0]!} number={1} layout={standaloneLayout} paneSize={gridSize} onChange={changePaneLayout} mobile={mobile} splitBlockedReason={panes.length > 1 ? "returnToGroup" : undefined} />
            </>}
            {activeSession && (
              <SessionToolsMenu
                  session={activeSession}
                  onChangeDock={!mobile ? () => onChangeDock(isBottom ? "right" : "bottom") : undefined}
                  dockLabel={isBottom ? t("connections.dockRight") : t("connections.dockBottom")}
                  onDetachAllSessions={!mobile && sessions.length > 1 ? onDetachAllSessions : undefined}
                  onDetachSession={onDetachSession}
                  onReconnectSession={handleReconnectSession}
                  onCloseSession={handleCloseSession}
                  onCloseAllSessions={handleCloseAllSessions}
                  isFileBrowserOpen={activeSessionFileBrowserOpen}
                  isRdpDriveReady={!!rdpFilesystems[activeSession.id]}
                  isVncSshFileBrowserReady={!!activeVncSshFileSession}
                  isClipboardReady={!!clipboardControllers[activeSession.id]}
                  onSendClipboard={
                    clipboardControllers[activeSession.id]
                      ? () => handleSendClipboard(activeSession)
                      : undefined
                  }
                  onToggleFileBrowser={() => {
                    if (activeSession.protocol === "vnc" && activeVncSshFileSession) {
                      setFileBrowserSessionId(activeVncSshFileSession.id);
                      onSelectSession(activeVncSshFileSession.id);
                      return;
                    }
                    setFileBrowserSessionId((current) =>
                      current === activeSession.id ? null : activeSession.id,
                    );
                  }}
                />
            )}
            {!activeSession && <Button
              variant="ghost" size="icon-sm"
              className={mobile ? "hidden" : undefined}
              onClick={() => onChangeDock(isBottom ? "right" : "bottom")}
              aria-label={isBottom ? t("connections.dockRight") : t("connections.dockBottom")}
              title={isBottom ? t("connections.dockRight") : t("connections.dockBottom")}
            >
              {isBottom ? <PanelRight className="h-4 w-4" /> : <PanelBottom className="h-4 w-4" />}
            </Button>}
            <Button className={mobile ? "hidden" : undefined} variant="ghost" size="icon-sm" onClick={onToggleExpand} aria-label={expanded ? t("connections.collapse") : t("connections.expand")} title={expanded ? t("connections.collapse") : t("connections.expand")}>
              {expanded ? <ChevronDown className="h-4 w-4" /> : <ChevronUp className="h-4 w-4" />}
            </Button>
          </div>
        </div>
        )}

        {/* Session content */}
        {sessions.length > 0 && (
          <div className={cn("flex-1 min-h-0 min-w-0 flex-col", expanded && !activePlaybookTab ? "flex" : "hidden")}>
            <div className={cn("min-h-0 min-w-0 flex-1", mobile ? "overflow-y-auto" : "overflow-hidden")}>
            <div ref={gridRef} data-session-pane-grid className="relative h-full min-h-0 min-w-0 overflow-hidden"
              style={{ "--session-pane-header-height": mobile ? "max(44px, 2.75rem)" : "2.25rem",
                minHeight: minimumGridHeight } as CSSProperties}>
              {showingGroup && <SessionPaneChrome layout={layout} sessions={sessions} onChange={changePaneLayout} onFocus={focusPane} gridSize={gridSize} mobile={mobile} renderHealth={renderSessionHealth}
                onResize={(event, split, rect) => {
                  const bounds = event.currentTarget.closest("[data-session-pane-grid]")?.getBoundingClientRect();
                  if (!bounds) return;
                  const { extent: size, min, max } = paneSplitBounds(split, rect, bounds);
                  if (size <= 0) return;
                  startPointerResize(event, split.direction === "horizontal" ? "x" : "y", delta => changePaneLayout(resizePaneSplit(layout, split.id, Math.max(min, Math.min(max, split.ratio + delta / size))), false), () => onPaneLayoutChange?.(layoutRef.current));
                }} />}
              {resizeCursor && (
                <div
                  aria-hidden="true"
                  className={cn(
                    "absolute inset-0 z-50 bg-transparent",
                    resizeCursor === "ew-resize" ? "cursor-ew-resize" : "cursor-ns-resize",
                  )}
                />
              )}
              {sessions.filter((session) => (
                session.status !== "idle" &&
                // A CCTV console renders a REST panel with no websocket/token, so
                // it is exempt from the ws-connectable gate the proxied sessions
                // must pass to mount their viewer.
                (session.protocol === "cctv" ||
                  isSessionWsConnectable(session.websocketUrl, session.token))
              )).map((session) => {
                const assignedPane = panes.find(pane => pane.sessionId === session.id);
                const isVisible = !activePlaybookTab && (showingGroup
                  ? !!assignedPane && (!layout.maximizedPaneId || assignedPane.id === layout.maximizedPaneId)
                  : session.id === activeSessionId);
                const isActive = isVisible && (!showingGroup || assignedPane?.id === layout.focusedPaneId) && visible && expanded;
                return (
                  <div
                    key={session.id}
                    className={cn(
                      "absolute min-h-0 min-w-0 overflow-hidden",
                      isVisible ? "z-10 visible" : "z-0 invisible pointer-events-none",
                      resizeCursor && "pointer-events-none",
                    )}
                    style={sessionRect(session.id)}
                    aria-hidden={!isVisible}
                    data-session-pane-viewer={session.id}
                    onPointerDownCapture={() => { if (assignedPane && !isActive) focusPane(assignedPane.id); }}
                  >
	                    {isVisible && !isActive && <button type="button" className="absolute inset-0 z-20 cursor-pointer bg-transparent" aria-label={t("connections.panes.focusSession", { protocol: session.protocol.toUpperCase(), host: session.hostName })} onClick={() => assignedPane && focusPane(assignedPane.id)} />}
	                    {(() => {
	                      const sessionRdpFilesystem =
	                        session.protocol === "rdp" ? rdpFilesystems[session.id] : undefined;
	                      const showFileBrowser =
	                        fileBrowserSessionId === session.id &&
	                        sessionSupportsFileBrowser(session);
	                      const fileBrowserPlacement = effectiveFileBrowserPlacement;
	                      const fileBrowserAvailable = fileBrowserAvailableSize(
	                        fileBrowserPlacement,
	                        fileBrowserSplitBounds,
	                      );
	                      const fileBrowserSize = clampSshFileBrowserSize(
	                        fileBrowserPlacement,
	                        fileBrowserState.sizes[fileBrowserPlacement],
	                        fileBrowserAvailable,
	                      );
	                      const fileBrowserMaxSize = getSshFileBrowserMaxSize(
	                        fileBrowserPlacement,
	                        fileBrowserAvailable,
	                      );

	                      return (
	                        <div
	                          inert={!isActive}
	                          ref={showFileBrowser && isActive ? setFileBrowserSplitElement : undefined}
	                          data-ssh-file-browser-split={showFileBrowser ? "" : undefined}
	                          className={cn(
	                            "flex h-full min-h-0 min-w-0",
                            showFileBrowser
                              ? fileBrowserPlacement === "right"
                                ? "flex-row"
                                : "flex-col"
                              : "flex-col",
                          )}
                        >
                          <div
                            className={cn(
                              "flex min-h-0 min-w-0 flex-col",
                              showFileBrowser ? "flex-1" : "h-full w-full",
                            )}
                          >
                            <div className="min-h-0 flex-1">
                              <LiveSessionViewer
                                sessionId={session.id}
                                protocol={session.protocol}
                                websocketUrl={session.websocketUrl}
                                token={session.token}
                                targetId={session.hostId}
                                  isActive={isActive && visible}
                                 isVisible={isVisible && visible && expanded}
                                onOpen={() => onSessionStatusChange(session.id, "connected")}
                                onClose={(clean, code, reason) => {
                                  if (session.status === "reconnecting") return;
                                  onSessionStatusChange(
                                    session.id,
                                    clean ? "disconnected" : "failed",
                                    { code, reason },
                                  );
                                }}
                                onReconnect={() => handleReconnectSession(session)}
                                onRdpFilesystem={
                                  session.protocol === "rdp"
                                    ? (fs) => handleRdpFilesystem(session.id, fs)
                                    : undefined
                                }
                                onClipboardControllerChange={
                                  session.protocol === "rdp" || session.protocol === "vnc"
                                    ? (controller) => handleClipboardController(session.id, controller)
                                    : undefined
                                }
                                onRemoteClipboardText={
                                  session.protocol === "rdp" || session.protocol === "vnc"
                                    ? (text) => handleRemoteClipboardText(session, text)
                                    : undefined
                                }
                              />
                            </div>
                          </div>
                          {showFileBrowser ? (
                            <>
	                              <div
	                                role="separator"
	                                aria-orientation={fileBrowserPlacement === "right" ? "vertical" : "horizontal"}
	                                aria-label={t("connections.toolsMenu.resizeFileBrowser")}
	                                aria-valuemin={SSH_FILE_BROWSER_MIN_SIZES[fileBrowserPlacement]}
	                                aria-valuemax={fileBrowserMaxSize}
	                                aria-valuenow={fileBrowserSize}
	                                aria-valuetext={`${fileBrowserSize}px`}
	                                tabIndex={0}
	                                className={cn(
	                                  "shrink-0 bg-border/55 transition-colors hover:bg-primary/35 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/60 active:bg-primary/45",
	                                  fileBrowserPlacement === "right"
	                                    ? "w-1.5 cursor-ew-resize"
	                                    : "h-1.5 cursor-ns-resize",
	                                )}
	                                onPointerDown={(event) => onFileBrowserResizePointerDown(event, fileBrowserPlacement)}
	                                onKeyDown={(event) =>
	                                  onFileBrowserResizeKeyDown(
	                                    event,
	                                    fileBrowserPlacement,
	                                    fileBrowserSize,
	                                    fileBrowserAvailable,
	                                  )
	                                }
	                              />
                              <div
                                className={cn(
                                  "min-h-0 min-w-0 overflow-hidden border-border/60 bg-background",
                                  fileBrowserPlacement === "right"
                                    ? "border-l"
                                    : "border-t",
                                )}
                                style={
                                  fileBrowserPlacement === "right"
                                    ? { width: fileBrowserSize }
                                    : { height: fileBrowserSize }
                                }
                              >
                                {session.protocol === "rdp" && sessionRdpFilesystem ? (
                                  <RdpFileBrowserPanel
                                    session={session}
                                    filesystem={sessionRdpFilesystem}
                                    placement={fileBrowserPlacement}
                                    onChangePlacement={(placement) => {
                                      updateFileBrowserState((current) => ({
                                        ...current,
                                        placement,
                                        sizes: {
                                          ...current.sizes,
                                          [placement]: clampSshFileBrowserSize(
                                            placement,
                                            current.sizes[placement],
                                          ),
                                        },
                                      }));
                                    }}
                                    onClose={() => setFileBrowserSessionId(null)}
                                  />
                                ) : (
                                  <SshFileBrowserPanel
                                    session={session}
                                    placement={fileBrowserPlacement}
                                    onChangePlacement={(placement) => {
                                      updateFileBrowserState((current) => ({
                                        ...current,
                                        placement,
                                        sizes: {
                                          ...current.sizes,
                                          [placement]: clampSshFileBrowserSize(
                                            placement,
                                            current.sizes[placement],
                                          ),
                                        },
                                      }));
                                    }}
                                    onClose={() => setFileBrowserSessionId(null)}
                                  />
                                )}
                              </div>
                            </>
                          ) : null}
                        </div>
                      );
                    })()}
                  </div>
                );
              })}
              {activeSession?.status === "connecting" && activeSessionCanConnect && (
                <div style={sessionRect(activeSession.id)} className="absolute z-20 flex flex-col items-center justify-center gap-3 bg-background p-4 text-center">
                  <Loader2
                    className="h-8 w-8 animate-spin text-primary"
                    data-testid="connections-session-connecting"
                  />
                  <div className="space-y-1">
                    <p className="text-sm font-medium text-foreground">
                      {t("connections.connectingSession")}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {activeSession.protocol.toUpperCase()} · {activeSession.hostName}
                    </p>
                  </div>
                </div>
              )}
              {activeSession && activeSession.protocol !== "cctv" &&
                activeSession.status !== "idle" &&
                !activeSessionCanConnect && (
                <div style={sessionRect(activeSession.id)} className="absolute z-20 flex flex-col items-center justify-center gap-3 bg-background p-4 text-center">
                  <Loader2
                    className="h-8 w-8 animate-spin text-status-warning"
                    data-testid="connections-session-restoring"
                  />
                  <div className="space-y-1">
                    <p className="text-sm font-medium text-foreground">
                      {t("connections.restoringSession")}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {activeSession.protocol.toUpperCase()} · {activeSession.hostName}
                    </p>
                  </div>
                </div>
              )}
              {/* Recovery overlay: a restored ("idle") session for any protocol, plus a
                  lost RDP/VNC canvas (the Guacamole/noVNC viewer can't surface a
                  half-open drop itself — health-driven `disconnected`/`failed`). SSH
                  keeps its own in-viewer reconnect overlay, so it is excluded here. */}
              {activeSession && (activeSession.status === "idle" ||
                ((activeSession.status === "disconnected" || activeSession.status === "failed") &&
                  (activeSession.protocol === "rdp" || activeSession.protocol === "vnc"))) && (
                <div style={sessionRect(activeSession.id)} className="absolute z-20 flex flex-col items-center justify-center gap-3 bg-background p-4 text-center">
                  <WifiOff className="h-8 w-8 text-muted-foreground" />
                  <div className="space-y-1">
                    <p className="text-sm font-medium text-foreground">
                      {/* A transport failure (incl. a half-open bridge death) reads
                          "Connection lost"; a clean end or a restored idle tab reads
                          "Session ended". */}
                      {t(
                        activeSession.status === "failed"
                          ? "connections.connectionLost"
                          : "connections.sessionEnded",
                      )}
                    </p>
                    <p className="text-xs text-muted-foreground">
                      {activeSession.protocol.toUpperCase()} · {activeSession.hostName}
                    </p>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-9 gap-2 text-sm"
                    onClick={() => handleReconnectSession(activeSession)}
                  >
                    <RotateCcw className="h-3.5 w-3.5" />
                    {t("connections.reconnectSession")}
                  </Button>
                </div>
              )}
            </div>
            </div>
          </div>
        )}

        {/* Playbook run tab content — single active viewer, unmounted on tab switch (unlike
            sessions, a playbook run has no client-side connection state to preserve). */}
        {activePlaybookTab && (
          <div className={cn("flex-1 min-h-0 min-w-0 flex-col", expanded ? "flex" : "hidden")}>
            <div className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
              <PlaybookTabViewer tab={activePlaybookTab} />
            </div>
          </div>
        )}
      </SurfaceCard>
    </div>
  );
};

export default ConnectionsPanel;
