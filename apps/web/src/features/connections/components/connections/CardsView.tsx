import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useVirtualizer } from "@tanstack/react-virtual";
import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import { Building2, Map as MapIcon, Network } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { connectionTargetFallbackIcon } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import { useAppliedUiScale } from "@/shared/hooks/useAppliedUiScale";
import type { ConnectionTarget } from "@/shared/api/types";
import { TargetBindingWarning } from "./TargetBindingWarning";
import { hostValue } from "./hostPresentation";
import QuickAccessButtons from "./QuickAccessButtons";
import type { TreeQuickAccessConfig } from "./treeConfig";
import type { PlaybookTargetAssignment } from "@/features/playbooks/api";
import { HostAvailabilityIndicator } from "@/features/observability/HostAvailabilityIndicator";
import { HostObservabilityCluster } from "@/features/observability/HostObservabilityHoverCard";
import type { HostLabelEntry } from "@/features/observability/api";
import type { OpenableProtocol } from "./launchPresentation";

interface Props {
  hosts: ConnectionTarget[];
  onRenderedHostIdsChange?: (ids: string[]) => void;
  selectedHost: ConnectionTarget | null;
  onSelectHost: (host: ConnectionTarget) => void;
  onConnect: (
    host: ConnectionTarget,
    protocol: OpenableProtocol,
    entryId?: string,
    username?: string,
  ) => void;
  onPlaybookQuickAction?: (host: ConnectionTarget, item: PlaybookTargetAssignment) => void;
  quickAccessConfig: TreeQuickAccessConfig;
  bulkSelectionMode?: boolean;
  bulkSelectedHostIds?: Set<string>;
  onBulkSelectionChange?: (keys: Set<string>) => void;
  hostLabels?: Map<string, HostLabelEntry>;
}

// Grid sizing constants in base pixels. They are multiplied by the active UI
// scale because card content uses rem/font-size based scaling.
const BASE_COLUMN_MIN_WIDTH = 220;
const BASE_GRID_GAP = 10; // matches gap-2.5 (0.625rem) at 16px root font size
const BASE_CARD_HEIGHT = 176; // includes readable metadata and a compact quick-action row

function columnsForWidth(width: number, minWidth: number, gap: number): number {
  if (width <= 0) return 1;
  // Mirror CSS auto-fill: floor((width + gap) / (minWidth + gap)).
  const cols = Math.floor((width + gap) / (minWidth + gap));
  return Math.max(1, cols);
}

const CardsView = ({
  hosts,
  selectedHost,
  onSelectHost,
  onConnect,
  onPlaybookQuickAction,
  quickAccessConfig,
  bulkSelectionMode = false,
  bulkSelectedHostIds = new Set(),
  onBulkSelectionChange,
  hostLabels,
  onRenderedHostIdsChange,
}: Props) => {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [columns, setColumns] = useState(1);
  const [quickAccessIntentStartedAt, setQuickAccessIntentStartedAt] = useState<Map<string, number>>(
    () => new Map(),
  );
  const uiScale = useAppliedUiScale();
  const gridGap = Math.ceil(BASE_GRID_GAP * uiScale);
  const columnMinWidth = Math.ceil(BASE_COLUMN_MIN_WIDTH * uiScale);
  const cardHeight = Math.ceil(BASE_CARD_HEIGHT * uiScale);

  const markQuickAccessIntent = (hostId: string) => {
    setQuickAccessIntentStartedAt((prev) => {
      if (prev.has(hostId)) return prev;
      const next = new Map(prev);
      next.set(hostId, Date.now());
      return next;
    });
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

  // Derive how many cards fit per row from the live container width so the
  // row-based virtualizer slices the flat host list into matching chunks.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => setColumns(columnsForWidth(el.clientWidth, columnMinWidth, gridGap));
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [columnMinWidth, gridGap]);

  const rowCount = Math.ceil(hosts.length / columns);

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual's useVirtualizer() returns measure/scroll functions that cannot be memoized safely; the React Compiler must skip this component instead of producing stale UI
  const rowVirtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => cardHeight + gridGap,
    overscan: 5,
  });

  useEffect(() => {
    rowVirtualizer.measure();
  }, [rowVirtualizer, cardHeight, gridGap]);
  const virtualRows = rowVirtualizer.getVirtualItems();
  const renderedHostIds = useMemo(
    () => virtualRows.flatMap((row) => hosts.slice(row.index * columns, (row.index + 1) * columns).map((host) => host.id)),
    [columns, hosts, virtualRows],
  );
  useEffect(() => {
    onRenderedHostIdsChange?.(renderedHostIds);
  }, [onRenderedHostIdsChange, renderedHostIds]);

  return (
    <div ref={scrollRef} className="flex-1 overflow-auto stable-scrollbar-gutter">
      <div className="relative w-full" style={{ height: rowVirtualizer.getTotalSize() }}>
        {virtualRows.map((virtualRow) => {
          const start = virtualRow.index * columns;
          const rowHosts = hosts.slice(start, start + columns);
          return (
            <div
              key={virtualRow.key}
              ref={rowVirtualizer.measureElement}
              data-index={virtualRow.index}
              className="absolute left-0 top-0 w-full grid gap-2.5"
              style={{
                transform: `translateY(${virtualRow.start}px)`,
                gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
                paddingBottom: gridGap,
              }}
            >
              {rowHosts.map((host) => {
                const isSelected = selectedHost?.id === host.id;
                const isBulkSelected = bulkSelectedHostIds.has(host.id);
                return (
                  <div
                    key={host.id}
                    className="group min-w-0"
                    data-testid={`host-card-${host.id}`}
                    onPointerEnter={() => markQuickAccessIntent(host.id)}
                    onFocusCapture={() => markQuickAccessIntent(host.id)}
                  >
                    <SurfaceCard
                      hover
                      selected={isSelected}
                      className={cn(
                        "group/card relative min-h-44 overflow-hidden p-3 pb-10 transition-colors duration-150 border border-border",
                        "hover:border-primary/30 hover:bg-accent",
                        isSelected &&
                          "border-primary bg-accent",
                      )}
                    >
                      <span
                        className={cn(
                          "absolute inset-x-0 top-0 h-0.5 transition-colors",
                          isSelected ? "bg-primary" : "bg-transparent group-hover/card:bg-primary/60",
                        )}
                      />
                      {bulkSelectionMode && (
                        <div
                          className="absolute right-3 top-3 z-10"
                          onClick={(event) => event.stopPropagation()}
                          onKeyDown={(event) => event.stopPropagation()}
                        >
                          <Checkbox
                            checked={isBulkSelected}
                            onCheckedChange={() => toggleBulkHost(host.id)}
                            aria-label={t("connections.bulkLaunch.selectTarget", { name: host.name })}
                          />
                        </div>
                      )}
                      <button
                        type="button"
                        onClick={() => onSelectHost(host)}
                        aria-label={t("connections.selectHost", { name: host.name })}
                        aria-pressed={isSelected}
                        className={cn(
                          "w-full text-left cursor-pointer outline-none rounded-md focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
                          bulkSelectionMode && "pr-8",
                        )}
                      >
                        <SurfaceCardHeader className="mb-2 border-0 p-0 md:p-0">
                          <div className="flex items-center gap-2 min-w-0">
                            <div
                              className={cn(
                                "w-7 h-7 rounded-md surface-inset flex items-center justify-center shrink-0 transition-colors",
                                isSelected && "bg-primary/12",
                              )}
                            >
                              <EntityIcon
                                fallback={connectionTargetFallbackIcon(host)}
                                iconName={host.roleIconName}
                                customDataUrl={host.roleIconDataUrl}
                                className="h-3.5 w-3.5 text-primary"
                                iconClassName="h-3.5 w-3.5"
                              />
                            </div>
                            <div className="min-w-0">
                              <div className="flex items-center gap-1 min-w-0">
                                <p className="text-sm font-medium text-foreground truncate">{host.name}</p>
                                <HostAvailabilityIndicator entry={hostLabels?.get(host.id)} />
                                <TargetBindingWarning host={host} />
                              </div>
                              <p className="text-xs text-muted-foreground truncate">{hostValue(host.role)}</p>
                              <HostObservabilityCluster
                                entry={hostLabels?.get(host.id)}
                                maxLabels={2}
                              />
                            </div>
                          </div>
                        </SurfaceCardHeader>

                        <SurfaceCardContent className="mb-2 flex flex-col gap-1 p-0 text-sm md:p-0">
                          <div className="flex items-center gap-1 text-muted-foreground">
                            <Network className="h-2.5 w-2.5" />
                            <span className="truncate font-mono text-[0.6875rem]">{hostValue(host.primaryIp)}</span>
                          </div>
                          <div className="flex items-center gap-1 text-muted-foreground">
                            <MapIcon className="h-2.5 w-2.5" />
                            <span className="truncate">{hostValue(host.region)}</span>
                          </div>
                          <div className="flex items-center gap-1 text-muted-foreground">
                            <Building2 className="h-2.5 w-2.5" />
                            <span className="truncate">{hostValue(host.site)}</span>
                          </div>
                        </SurfaceCardContent>
                      </button>
                      <QuickAccessButtons
                        host={host}
                        config={quickAccessConfig}
                        intent={quickAccessIntentStartedAt.has(host.id)}
                        intentStartedAt={quickAccessIntentStartedAt.get(host.id) ?? null}
                        onIntent={() => markQuickAccessIntent(host.id)}
                        onConnect={onConnect}
                        onPlaybookQuickAction={onPlaybookQuickAction}
                        variant="card"
                      />
                    </SurfaceCard>
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
};

export default CardsView;
