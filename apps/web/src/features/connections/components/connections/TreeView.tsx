import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import { Tooltip, TooltipTrigger, TooltipContent } from "@/shared/ui-primitives/tooltip";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { cn } from "@/shared/lib/utils";
import { connectionTargetFallbackIcon, treeGroupIconFor } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import type { ConnectionTarget } from "@/shared/api/types";
import type { TreeNode } from "./TreeNodeInspector";
import { AUTO_PREFERRED_ORDER, type TreeConfig, type TreeGroupKey } from "./treeConfig";
import { TargetBindingWarning } from "./TargetBindingWarning";
import { hostSubtitle, hostValue } from "./hostPresentation";
import { useSetting } from "@/shared/settings";
import { useAppliedUiScale } from "@/shared/hooks/useAppliedUiScale";
import QuickAccessButtons from "./QuickAccessButtons";
import type { PlaybookTargetAssignment } from "@/features/playbooks/api";
import { HostAvailabilityIndicator } from "@/features/observability/HostAvailabilityIndicator";
import { HostObservabilityCluster } from "@/features/observability/HostObservabilityHoverCard";
import type { HostLabelEntry } from "@/features/observability/api";
import type { OpenableProtocol } from "./launchPresentation";
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
const TREE_ROW_BASE_PADDING = 8;
const TREE_ROW_DEPTH_INDENT = 10;

function treeRowPaddingLeft(depth: number): number {
  return TREE_ROW_BASE_PADDING + Math.min(depth, 3) * TREE_ROW_DEPTH_INDENT;
}

function HostRow({
  host,
  depth,
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
}: {
  host: ConnectionTarget;
  depth: number;
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
}) {
  const { t } = useTranslation();
  const [quickAccessIntentStartedAt, setQuickAccessIntentStartedAt] = useState<number | null>(null);
  const markQuickAccessIntent = () => {
    setQuickAccessIntentStartedAt((prev) => prev ?? Date.now());
  };

  return (
    <Tooltip>
        <div
          role="button"
          tabIndex={0}
          onPointerEnter={markQuickAccessIntent}
          onFocusCapture={markQuickAccessIntent}
          onClick={() => onSelect(host)}
          onKeyDown={(event) => {
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            onSelect(host);
          }}
          className={cn(
            "connection-tree-row group relative flex items-center gap-2 w-full min-h-10 py-1 pr-2 text-sm rounded-md transition-colors",
            (selectedHost?.id === host.id ||
              selectedNode?.id === host.id)
              ? "bg-primary/8 text-primary shadow-sm"
              : "text-foreground hover:bg-accent",
          )}
          style={{ paddingLeft: treeRowPaddingLeft(depth) }}
        >
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
            intent={quickAccessIntentStartedAt !== null}
            intentStartedAt={quickAccessIntentStartedAt}
            onIntent={markQuickAccessIntent}
            variant="tree"
            onConnect={onConnect}
            onPlaybookQuickAction={onPlaybookQuickAction}
          />
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

interface GroupNode {
  key: string;          // stable path-id, e.g. "region=EU|site=DC-1"
  label: string;        // display label
  groupKey: TreeGroupKey; // which dimension
  hosts: ConnectionTarget[];
  children: GroupNode[];
}

type VisibleTreeRow =
  | { kind: "group"; node: GroupNode; depth: number; isOpen: boolean }
  | { kind: "host"; host: ConnectionTarget; depth: number };

function readField(h: ConnectionTarget, key: TreeGroupKey): string {
  const v = (h as unknown as Record<string, unknown>)[key];
  return typeof v === "string" && v ? v : "";
}

function buildGroups(
  hosts: ConnectionTarget[],
  order: TreeGroupKey[],
  includeUnknown: boolean,
  pathPrefix = "",
): GroupNode[] {
  const [head, ...rest] = order;
  if (head === undefined) return [];
  const bucket = new Map<string, ConnectionTarget[]>();
  for (const h of hosts) {
    const raw = readField(h, head);
    const label = raw || "—";
    if (!raw && !includeUnknown) continue;
    const list = bucket.get(label) ?? [];
    list.push(h);
    bucket.set(label, list);
  }
  const nodes: GroupNode[] = [];
  const labels = Array.from(bucket.keys()).sort((a, b) => a.localeCompare(b));
  for (const label of labels) {
    const groupHosts = bucket.get(label) ?? [];
    const key = `${pathPrefix}${head}=${label}|`;
    nodes.push({
      key,
      label,
      groupKey: head,
      hosts: groupHosts,
      children: buildGroups(groupHosts, rest, includeUnknown, key),
    });
  }
  return nodes;
}

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
  const rowHeight = Math.ceil(TREE_ROW_BASE_HEIGHT * uiScale);
  const order: TreeGroupKey[] = useMemo(() => {
    if (config.mode === "custom") return config.order;
    // Auto: keep only dimensions that have at least one non-empty value.
    return AUTO_PREFERRED_ORDER.filter((key) => hosts.some((h) => readField(h, key)));
  }, [config, hosts]);

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
    setPersistedCollapsedGroups((prev: unknown) =>
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
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    getItemKey: getRowKey,
    overscan: TREE_ROWS_OVERSCAN,
  });
  const virtualRows = virtualizer.getVirtualItems();
  const renderedHostIds = useMemo(() => {
    const rows = shouldVirtualize ? virtualRows.map((row) => visibleRows[row.index]) : visibleRows;
    return rows.flatMap((row) => row?.kind === "host" ? [row.host.id] : []);
  }, [shouldVirtualize, virtualRows, visibleRows]);
  useEffect(() => {
    onRenderedHostIdsChange?.(renderedHostIds);
  }, [onRenderedHostIdsChange, renderedHostIds]);

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
    if (onSelectNode) {
      onSelectNode({ type: "host", id: host.id, name: host.name, host, hosts: [host] });
    }
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

  const renderRow = (row: VisibleTreeRow) => {
    if (row.kind === "host") {
      return (
        <HostRow
          host={row.host}
          depth={row.depth}
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
        />
      );
    }

    const { node, depth, isOpen } = row;
    const roleIconHost =
      node.groupKey === "role"
        ? node.hosts.find((host) => host.roleIconDataUrl || host.roleIconName)
        : undefined;
    const selectedHere = selectedNode?.id === node.key;
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
          "connection-tree-row flex items-center gap-2 w-full min-h-10 py-1 pr-2 text-sm rounded-md transition-colors",
          selectedHere
            ? "bg-primary/8 text-primary"
            : "text-foreground hover:bg-accent",
          depth === 0 && "font-medium",
        )}
        style={{ paddingLeft: treeRowPaddingLeft(depth) }}
      >
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            toggle(node.key);
          }}
          className="-my-1 -ml-1 inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-sm hover:text-primary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-primary"
          aria-label={`${isOpen ? t("connections.collapse") : t("connections.expand")} ${node.label}`}
          aria-expanded={isOpen}
        >
          {isOpen ? (
            <ChevronDown className="h-3 w-3 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-3 w-3 text-muted-foreground" />
          )}
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
        <button
          type="button"
          className="flex items-center gap-1.5 flex-1 min-w-0 text-left focus-visible:outline-none"
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
      </div>
    );
  };

  return (
    <SurfaceCard className="@container flex-1 min-h-0 overflow-hidden p-2">
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
                  className="connection-tree-virtual-row absolute left-0 top-0 w-full"
                  style={{ height: virtualRow.size, transform: `translateY(${virtualRow.start}px)` }}
                >
                  {renderRow(row)}
                </div>
              );
            })}
          </div>
        </div>
      ) : (
        <div ref={scrollRef} className="connection-tree-scroll h-full overflow-auto stable-scrollbar-gutter space-y-px">
          {visibleRows.map((row) => {
            const key =
              row.kind === "group"
                ? `group:${row.node.key}`
                : `host:${row.host.id}`;
            return <div key={key}>{renderRow(row)}</div>;
          })}
        </div>
      )}
    </SurfaceCard>
  );
};

export default TreeView;
