import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import { TreeRowGuides } from "@/shared/ui-shared/TreeRowGuides";
import { TREE_ROW_CONTENT_STYLE, treeRowPaths, treeRowStyle } from "@/shared/ui-shared/treeRowGeometry";
import { Tooltip, TooltipTrigger, TooltipContent } from "@/shared/ui-primitives/tooltip";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { cn } from "@/shared/lib/utils";
import { connectionTargetFallbackIcon, treeGroupIconFor } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import type { ConnectionTarget } from "@/shared/api/types";
import type { TreeNode } from "./TreeNodeInspector";
import { type TreeConfig } from "./treeConfig";
import { buildGroups, treeGroupOrder, type GroupNode } from "./treeGroups";
import { TargetBindingWarning } from "./TargetBindingWarning";
import { hostSubtitle, hostValue } from "./hostPresentation";
import { useSetting } from "@/shared/settings";
import { useAppliedUiScale } from "@/shared/hooks/useAppliedUiScale";
import QuickAccessButtons from "./QuickAccessButtons";
import { useQuickAccessIntent } from "./useQuickAccessIntent";
import type { PlaybookTargetAssignment } from "@/features/playbooks/api";
import { HostAvailabilityIndicator } from "@/features/observability/HostAvailabilityIndicator";
import { HostObservabilityCluster } from "@/features/observability/HostObservabilityHoverCard";
import type { HostLabelEntry } from "@/features/observability/api";
import type { OpenableProtocol } from "./launchPresentation";
import { useHostReveal } from "../../hooks/useHostReveal";
import {
  collapsedTreeGroupSetForScope,
  EMPTY_TREE_COLLAPSED_GROUPS,
  normalizeTreeCollapsedGroups,
  readRetiredCollapsedTreeGroups,
  TREE_COLLAPSED_PREFERENCE_KEY,
  treeCollapseScope,
  withCollapsedTreeGroupScope,
} from "./treeCollapseState";

const TREE_ROWS_VIRTUALIZE_THRESHOLD = 160;
const TREE_ROW_BASE_HEIGHT = 40;
const TREE_ROWS_OVERSCAN = 8;

function HostRow({
  host,
  depth,
  path,
  selectedHost,
  selectedNode,
  onSelect,
  onConnect,
  onPlaybookQuickAction,
  config,
  bulkSelectionMode,
  bulkSelected,
  onToggleBulkSelected,
  hostLabels,
  activeRails,
  activeBranch,
  intentStartedAt,
  onIntent,
}: {
  host: ConnectionTarget;
  depth: number;
  path: boolean[];
  selectedHost: ConnectionTarget | null;
  selectedNode?: TreeNode | null;
  onSelect: (host: ConnectionTarget) => void;
  onConnect: (host: ConnectionTarget, protocol: OpenableProtocol, entryId?: string, username?: string) => void;
  onPlaybookQuickAction?: (host: ConnectionTarget, item: PlaybookTargetAssignment) => void;
  config: TreeConfig;
  bulkSelectionMode: boolean;
  bulkSelected: boolean;
  onToggleBulkSelected?: (hostId: string) => void;
  hostLabels?: Map<string, HostLabelEntry>;
  activeRails: { level: number; top: boolean; bottom: boolean }[];
  activeBranch: boolean;
  /** Timestamp of row hover/focus intent, already scoped to this host. */
  intentStartedAt: number | null;
  onIntent: () => void;
}) {
  const { t } = useTranslation();

  return (
    <Tooltip>
        <div
          role="button"
          tabIndex={0}
          onClick={() => onSelect(host)}
          onPointerEnter={onIntent}
          onFocusCapture={onIntent}
          onKeyDown={(event) => {
            if (event.target instanceof Element && event.target.closest("button, input, [role=checkbox], a")) return;
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            onSelect(host);
          }}
          className={cn(
            "connection-tree-row group relative flex items-center w-full min-h-10 py-1 pr-2 text-sm rounded-md transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            (selectedNode ? selectedNode.type === "host" && selectedNode.id === host.id : selectedHost?.id === host.id)
              ? "bg-primary/8 text-primary shadow-sm"
              : "text-foreground hover:bg-accent",
          )}
          style={treeRowStyle(depth)}
        >
          <TreeRowGuides path={path} expanded={false} activeRails={activeRails} activeBranch={activeBranch} />
          <span aria-hidden="true" className="h-6 w-6 shrink-0" />
          <EntityIcon
            fallback={connectionTargetFallbackIcon(host)}
            iconName={host.roleIconName}
            customDataUrl={host.roleIconDataUrl}
            className="h-3 w-3 text-muted-foreground"
            iconClassName="h-3 w-3"
          />
          <TooltipTrigger asChild>
            <span tabIndex={0} className="min-w-0 flex-1 truncate rounded-sm font-medium outline-none focus-visible:ring-2 focus-visible:ring-ring" title={host.name}>{host.name}</span>
          </TooltipTrigger>
           <HostAvailabilityIndicator entry={hostLabels?.get(host.id)} className="border-transparent bg-transparent px-0 [&>span:last-child]:hidden @[400px]:[&>span:last-child]:inline" />
          <TargetBindingWarning host={host} />
          <HostObservabilityCluster
            entry={hostLabels?.get(host.id)}
            className="hidden @[400px]:inline-flex"
          />
          <QuickAccessButtons
            host={host}
            config={config.quickAccess}
            variant="tree"
            intentStartedAt={intentStartedAt}
            onIntent={onIntent}
            onConnect={onConnect}
            onPlaybookQuickAction={onPlaybookQuickAction}
          />
          {bulkSelectionMode && (
            <span
              className="-my-1 flex h-6 w-6 shrink-0 items-center justify-center"
              onClick={(event) => event.stopPropagation()}
              onKeyDown={(event) => event.stopPropagation()}
            >
              <Checkbox
                checked={bulkSelected}
                onCheckedChange={() => onToggleBulkSelected?.(host.id)}
                aria-label={t("connections.bulkLaunch.selectTarget", { name: host.name })}
              />
            </span>
          )}
        </div>
      <TooltipContent side="right" className="text-xs">
        <div className="space-y-0.5">
          <div className="font-medium">{host.name}</div>
          <div className="text-muted-foreground">
            {hostSubtitle([host.role, host.platform])}
          </div>
          <div className="font-mono text-xs">{hostValue(host.primaryIp)}</div>
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

interface Props {
  hosts: ConnectionTarget[];
  onRenderedHostIdsChange?: (ids: string[]) => void;
  selectedHost: ConnectionTarget | null;
  onSelectHost: (host: ConnectionTarget) => void;
  revealHostId?: string | null;
  revealNavigationKey?: string;
  revealReady?: boolean;
  onConnect: (host: ConnectionTarget, protocol: OpenableProtocol, entryId?: string, username?: string) => void;
  onPlaybookQuickAction?: (host: ConnectionTarget, item: PlaybookTargetAssignment) => void;
  selectedNode?: TreeNode | null;
  onSelectNode?: (node: TreeNode) => void;
  config: TreeConfig;
  bulkSelectionMode?: boolean;
  bulkSelectedHostIds?: Set<string>;
  onBulkSelectionChange?: (keys: Set<string>) => void;
  searchActive?: boolean;
  hostLabels?: Map<string, HostLabelEntry>;
}

type VisibleTreeRow =
  | { kind: "group"; node: GroupNode; depth: number; isOpen: boolean }
  | { kind: "host"; host: ConnectionTarget; depth: number };

function flattenVisibleRows(
  nodes: GroupNode[],
  collapsedGroups: Set<string>,
  depth = 0,
): VisibleTreeRow[] {
  const rows: VisibleTreeRow[] = [];
  for (const node of nodes) {
    const isOpen = !collapsedGroups.has(node.key);
    rows.push({ kind: "group", node, depth, isOpen });
    if (!isOpen) continue;
    if (node.children.length > 0) {
      rows.push(...flattenVisibleRows(node.children, collapsedGroups, depth + 1));
    } else {
      rows.push(...node.hosts.map((host) => ({ kind: "host" as const, host, depth: depth + 1 })));
    }
  }
  return rows;
}

function collectGroupKeys(nodes: GroupNode[], keys = new Set<string>()): Set<string> {
  for (const node of nodes) {
    keys.add(node.key);
    collectGroupKeys(node.children, keys);
  }
  return keys;
}

function selectedGroupPath(nodes: GroupNode[], node: TreeNode | null | undefined, hostId: string | undefined): GroupNode[] | null {
  for (const group of nodes) {
    if (node && node.type !== "host" && node.id === group.key && node.type === group.groupKey) return [group];
    const childPath = selectedGroupPath(group.children, node, hostId);
    if (childPath) return [group, ...childPath];
    if (!group.children.length && hostId && group.hosts.some((host) => host.id === hostId)) return [group];
  }
  return null;
}

function areSetsEqual(left: Set<string>, right: Set<string>): boolean {
  if (left.size !== right.size) return false;
  for (const value of left) {
    if (!right.has(value)) return false;
  }
  return true;
}

const TreeView = ({
  hosts,
  selectedHost,
  onSelectHost,
  revealHostId,
  revealNavigationKey,
  revealReady = true,
  onConnect,
  onPlaybookQuickAction,
  selectedNode,
  onSelectNode,
  config,
  bulkSelectionMode = false,
  bulkSelectedHostIds = new Set(),
  onBulkSelectionChange,
  searchActive = false,
  hostLabels,
  onRenderedHostIdsChange,
}: Props) => {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);
  const uiScale = useAppliedUiScale();
  const { intentStartedAt: quickAccessIntentStartedAt, markIntent: markQuickAccessIntent } = useQuickAccessIntent();
  const rowHeight = Math.ceil(TREE_ROW_BASE_HEIGHT * uiScale);
  const order = useMemo(() => treeGroupOrder(hosts, config), [config, hosts]);

  const tree = useMemo(
    () => buildGroups(hosts, order, config.includeUnknown),
    [hosts, order, config.includeUnknown],
  );
  const validGroupKeys = useMemo(() => collectGroupKeys(tree), [tree]);
  const collapseScope = useMemo(
    () => treeCollapseScope(order, config.includeUnknown),
    [config.includeUnknown, order],
  );
  const {
    value: persistedCollapsedGroups,
    setValue: setPersistedCollapsedGroups,
    isLoading: collapsedGroupsLoading,
  } = useSetting<unknown>(
    TREE_COLLAPSED_PREFERENCE_KEY,
    EMPTY_TREE_COLLAPSED_GROUPS,
    { debounceMs: 150, normalize: normalizeTreeCollapsedGroups },
  );
  const retiredCollapsedGroups = useMemo(
    () => readRetiredCollapsedTreeGroups(),
    [],
  );
  const persistedCollapsedGroupSet = useMemo(
    () =>
      collapsedTreeGroupSetForScope(
        persistedCollapsedGroups,
        collapseScope,
        validGroupKeys,
        retiredCollapsedGroups,
      ),
    [collapseScope, retiredCollapsedGroups, persistedCollapsedGroups, validGroupKeys],
  );
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const [searchCollapsedGroups, setSearchCollapsedGroups] = useState<Set<string>>(() => new Set());
  const collapseScopeRef = useRef(collapseScope);
  const collapsedGroupsTouchedRef = useRef(false);

  useEffect(() => {
    if (!searchActive) return;
    setSearchCollapsedGroups(new Set());
  }, [searchActive]);

  // Hydrate collapsed branches after preferences and the current tree structure are known.
  useEffect(() => {
    if (searchActive) return;
    const scopeChanged = collapseScopeRef.current !== collapseScope;
    if (scopeChanged) {
      collapseScopeRef.current = collapseScope;
      collapsedGroupsTouchedRef.current = false;
    }
    if (collapsedGroupsLoading) return;
    if (collapsedGroupsTouchedRef.current && !scopeChanged) return;
    setCollapsedGroups((current) =>
      areSetsEqual(current, persistedCollapsedGroupSet)
        ? current
        : persistedCollapsedGroupSet,
    );
  }, [collapseScope, collapsedGroupsLoading, persistedCollapsedGroupSet, searchActive]);

  const persistCollapsedGroups = (next: Set<string>) => {
    void setPersistedCollapsedGroups((prev: unknown) =>
      withCollapsedTreeGroupScope(prev, collapseScope, next, validGroupKeys),
    );
  };

  const effectiveCollapsedGroups = searchActive ? searchCollapsedGroups : collapsedGroups;
  const visibleRows = useMemo<VisibleTreeRow[]>(() => {
    if (order.length === 0) {
      return hosts.map((host) => ({ kind: "host" as const, host, depth: 0 }));
    }
    return flattenVisibleRows(tree, effectiveCollapsedGroups);
  }, [effectiveCollapsedGroups, hosts, order.length, tree]);
  const rowPaths = useMemo(() => treeRowPaths(visibleRows.map((row) => row.depth)), [visibleRows]);
  const selectionPath = useMemo(() => {
    const groupSelection = selectedNode && selectedNode.type !== "host" ? selectedNode : null;
    const hostId = groupSelection ? undefined : selectedNode?.type === "host" ? selectedNode.id : selectedHost?.id;
    const groups = selectedGroupPath(tree, groupSelection, hostId) ?? [];
    const groupKeys = new Set(groups.map((group) => group.key));
    const indices: number[] = [];
    visibleRows.forEach((row, index) => {
      if (row.kind === "group" ? groupKeys.has(row.node.key) : !!hostId && row.host.id === hostId) indices.push(index);
    });
    const rails = visibleRows.map(() => [] as { level: number; top: boolean; bottom: boolean }[]);
    for (let i = 0; i + 1 < indices.length; i++) {
      const start = indices[i]!;
      const end = indices[i + 1]!;
      const level = visibleRows[start]!.depth;
      for (let row = start; row <= end; row++) rails[row]!.push({ level, top: row !== start, bottom: row !== end });
    }
    return { groupKeys, indices: new Set(indices), rails };
  }, [selectedHost?.id, selectedNode, tree, visibleRows]);
  const shouldVirtualize = visibleRows.length > TREE_ROWS_VIRTUALIZE_THRESHOLD;
  const getRowKey = useCallback(
    (index: number) => {
      const row = visibleRows[index];
      if (!row) return `missing:${index}`;
      return row.kind === "group"
        ? `group:${row.node.key}`
        : `host:${row.host.id}`;
    },
    [visibleRows],
  );

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual's useVirtualizer() owns imperative scroll/measure state outside React's memo model, so its returned functions cannot be memoized safely
  const virtualizer = useVirtualizer({
    count: shouldVirtualize ? visibleRows.length : 0,
    useFlushSync: false,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    getItemKey: getRowKey,
    overscan: TREE_ROWS_OVERSCAN,
  });
  useEffect(() => {
    virtualizer.measure();
  }, [rowHeight, virtualizer]);
  const virtualRows = virtualizer.getVirtualItems();
  const renderedHostIds = useMemo(() => {
    const rows = shouldVirtualize ? virtualRows.map((row) => visibleRows[row.index]) : visibleRows;
    return rows.flatMap((row) => row?.kind === "host" ? [row.host.id] : []);
  }, [shouldVirtualize, virtualRows, visibleRows]);
  useEffect(() => {
    onRenderedHostIdsChange?.(renderedHostIds);
  }, [onRenderedHostIdsChange, renderedHostIds]);

  const revealHost = useCallback(() => {
    if (!revealReady || !revealHostId || !scrollRef.current) return false;
    if (!searchActive && (collapsedGroupsLoading ||
      (!collapsedGroupsTouchedRef.current && !areSetsEqual(collapsedGroups, persistedCollapsedGroupSet)))) return false;
    const groups = selectedGroupPath(tree, null, revealHostId);
    if (order.length > 0 && !groups) return false;
    const ancestors = new Set(groups?.map((group) => group.key));
    if ([...ancestors].some((key) => effectiveCollapsedGroups.has(key))) {
      const expanded = new Set([...effectiveCollapsedGroups].filter((key) => !ancestors.has(key)));
      if (searchActive) setSearchCollapsedGroups(expanded);
      else {
        collapsedGroupsTouchedRef.current = true;
        setCollapsedGroups(expanded);
      }
      return false;
    }
    const index = visibleRows.findIndex((row) => row.kind === "host" && row.host.id === revealHostId);
    if (index < 0) return false;
    if (shouldVirtualize) virtualizer.scrollToIndex(index, { align: "auto" });
    else {
      const row = scrollRef.current.querySelectorAll<HTMLElement>(".connection-tree-row")[index];
      if (!row) return false;
      row.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
    return true;
  }, [collapsedGroups, collapsedGroupsLoading, effectiveCollapsedGroups, order.length, persistedCollapsedGroupSet,
    revealHostId, revealReady, searchActive, shouldVirtualize, tree, virtualizer, visibleRows]);
  useHostReveal(revealHostId, revealHost, { navigationKey: revealNavigationKey });

  const toggle = (k: string) => {
    if (searchActive) {
      setSearchCollapsedGroups((s) => {
        const n = new Set(s);
        if (n.has(k)) n.delete(k);
        else n.add(k);
        return n;
      });
      return;
    }
    setCollapsedGroups((s) => {
      const n = new Set(s);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      collapsedGroupsTouchedRef.current = true;
      persistCollapsedGroups(n);
      return n;
    });
  };

  const selectGroup = (node: GroupNode) => {
    if (!onSelectNode) return;
    onSelectNode({
      type: node.groupKey,
      id: node.key,
      name: node.label,
      hosts: node.hosts,
    });
  };

  const toggleAndSelectGroup = (node: GroupNode) => {
    toggle(node.key);
    selectGroup(node);
  };

  const selectHost = (host: ConnectionTarget) => {
    // onSelectHost updates both the selected node and the host in the workspace;
    // calling onSelectNode too would duplicate the same URL write.
    onSelectHost(host);
  };

  const toggleBulkHost = (hostId: string) => {
    if (!onBulkSelectionChange) return;
    const next = new Set(bulkSelectedHostIds);
    if (next.has(hostId)) {
      next.delete(hostId);
    } else {
      next.add(hostId);
    }
    onBulkSelectionChange(next);
  };

  const setBulkHosts = (hostsToSet: ConnectionTarget[], selected: boolean) => {
    if (!onBulkSelectionChange) return;
    const next = new Set(bulkSelectedHostIds);
    for (const host of hostsToSet) {
      if (selected) {
        next.add(host.id);
      } else {
        next.delete(host.id);
      }
    }
    onBulkSelectionChange(next);
  };

  const renderRow = (row: VisibleTreeRow, index: number) => {
    if (row.kind === "host") {
      return (
        <HostRow
          host={row.host}
          depth={row.depth}
          path={rowPaths[index] ?? []}
          activeRails={selectionPath.rails[index] ?? []}
          activeBranch={selectionPath.indices.has(index)}
          selectedHost={selectedHost}
          selectedNode={selectedNode}
          onSelect={selectHost}
          onConnect={onConnect}
          onPlaybookQuickAction={onPlaybookQuickAction}
          config={config}
          bulkSelectionMode={bulkSelectionMode}
          bulkSelected={bulkSelectedHostIds.has(row.host.id)}
          onToggleBulkSelected={toggleBulkHost}
          hostLabels={hostLabels}
          intentStartedAt={quickAccessIntentStartedAt(row.host.id)}
          onIntent={() => markQuickAccessIntent(row.host.id)}
        />
      );
    }

    const { node, depth, isOpen } = row;
    const roleIconHost =
      node.groupKey === "role"
        ? node.hosts.find((host) => host.roleIconDataUrl || host.roleIconName)
        : undefined;
    const selectedHere = selectedNode?.type === node.groupKey && selectedNode.id === node.key;
    const ancestorHere = !selectedHere && selectionPath.groupKeys.has(node.key);
    const selectedInGroup = node.hosts.filter((host) => bulkSelectedHostIds.has(host.id)).length;
    const groupBulkSelected =
      selectedInGroup === 0
        ? false
        : selectedInGroup === node.hosts.length
          ? true
          : "indeterminate";
    return (
      <div
        className={cn(
          "connection-tree-row relative flex items-center w-full min-h-10 py-1 pr-2 text-sm rounded-md transition-colors",
          selectedHere
            ? "bg-primary/8 text-primary"
            : ancestorHere ? "bg-primary/5 text-primary/80 hover:bg-accent" : "text-foreground hover:bg-accent",
          depth === 0 && "font-medium",
        )}
        style={treeRowStyle(depth)}
      >
        <TreeRowGuides path={rowPaths[index] ?? []} expanded={isOpen && (node.children.length > 0 || node.hosts.length > 0)} activeRails={selectionPath.rails[index]} activeBranch={selectionPath.indices.has(index)} />
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            toggle(node.key);
          }}
          className="relative -my-1 inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-sm hover:text-primary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-primary"
          aria-label={`${isOpen ? t("connections.collapse") : t("connections.expand")} ${node.label}`}
          aria-expanded={isOpen}
        >
          {isOpen ? (
            <ChevronDown className="h-3 w-3 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-3 w-3 text-muted-foreground" />
          )}
        </button>
        <button
          type="button"
          className="flex items-center flex-1 min-w-0 rounded-sm text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          style={TREE_ROW_CONTENT_STYLE}
          onClick={() => toggleAndSelectGroup(node)}
          aria-expanded={isOpen}
        >
          <EntityIcon
            fallback={treeGroupIconFor(node.groupKey)}
            iconName={roleIconHost?.roleIconName}
            customDataUrl={roleIconHost?.roleIconDataUrl}
            className="h-3 w-3 text-primary"
            iconClassName="h-3 w-3"
          />
          <span className="truncate" title={node.label}>
            {node.label}
          </span>
           <span className="hidden text-xs text-muted-foreground/70 ml-1 @[480px]:inline">
            {t(`connections.treeSettings.groups.${node.groupKey}`, node.groupKey)}
          </span>
          <span className="ml-auto text-xs text-muted-foreground shrink-0">
            {node.hosts.length}
          </span>
        </button>
        {bulkSelectionMode && (
          <span
            className="-my-1 flex h-6 w-6 shrink-0 items-center justify-center"
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => event.stopPropagation()}
          >
            <Checkbox
              checked={groupBulkSelected}
              onCheckedChange={(checked) => setBulkHosts(node.hosts, checked === true)}
              aria-label={t("connections.bulkLaunch.selectGroup", { name: node.label })}
            />
          </span>
        )}
      </div>
    );
  };

  return (
    <SurfaceCard className="@container flex-1 min-h-0 overflow-hidden p-0">
      {visibleRows.length === 0 ? (
        <div className="flex items-center justify-center py-8 text-xs text-muted-foreground">
          {t("connections.treeSettings.noGroups")}
        </div>
      ) : shouldVirtualize ? (
        <div ref={scrollRef} className="connection-tree-scroll h-full overflow-auto stable-scrollbar-gutter">
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {virtualRows.map((virtualRow) => {
              const row = visibleRows[virtualRow.index];
              if (!row) return null;
              return (
                <div
                  key={virtualRow.key}
                  data-index={virtualRow.index}
                  ref={virtualizer.measureElement}
                  className="connection-tree-virtual-row absolute left-0 top-0 w-full"
                  style={{ transform: `translateY(${virtualRow.start}px)` }}
                >
                  {renderRow(row, virtualRow.index)}
                </div>
              );
            })}
          </div>
        </div>
      ) : (
        <div ref={scrollRef} className="connection-tree-scroll h-full overflow-auto stable-scrollbar-gutter">
          {visibleRows.map((row, index) => {
            const key =
              row.kind === "group"
                ? `group:${row.node.key}`
                : `host:${row.host.id}`;
            return <div key={key}>{renderRow(row, index)}</div>;
          })}
        </div>
      )}
    </SurfaceCard>
  );
};

export default TreeView;
