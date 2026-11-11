import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useVirtualizer } from "@tanstack/react-virtual";
import SurfaceCard, { SurfaceCardContent, SurfaceCardFooter, SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import { Map as MapIcon, Network } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { cn } from "@/shared/lib/utils";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { connectionTargetFallbackIcon } from "@/shared/entity-icon-utils";
import { EntityIcon } from "@/shared/entity-icons";
import { useAppliedUiScale } from "@/shared/hooks/useAppliedUiScale";
import type { ConnectionTarget } from "@/shared/api/types";
import { TargetBindingWarning } from "./TargetBindingWarning";
import { hostSubtitle, hostValue } from "./hostPresentation";
import { HostStatusValue } from "./HostStatus";
import { inventoryResourceStatusLabel } from "@/features/inventory/resourceStatus";
import QuickAccessButtons from "./QuickAccessButtons";
import { useQuickAccessIntent } from "./useQuickAccessIntent";
import type { TreeQuickAccessConfig } from "./treeConfig";
import type { PlaybookTargetAssignment } from "@/features/playbooks/api";
import { HostAvailabilityIndicator } from "@/features/observability/HostAvailabilityIndicator";
import { HostObservabilityCluster } from "@/features/observability/HostObservabilityHoverCard";
import type { HostLabelEntry } from "@/features/observability/api";
import type { OpenableProtocol } from "./launchPresentation";
import { useHostReveal } from "../../hooks/useHostReveal";

interface Props {
  hosts: ConnectionTarget[];
  onRenderedHostIdsChange?: (ids: string[]) => void;
  selectedHost: ConnectionTarget | null;
  revealHostId?: string | null;
  revealNavigationKey?: string;
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
const BASE_COLUMN_MIN_WIDTH = 280;
const BASE_GRID_GAP = 16;
const BASE_CARD_HEIGHT = 240; // estimated height; measured rows accommodate wrapped names

function columnsForWidth(width: number, minWidth: number, gap: number): number {
  if (width <= 0) return 1;
  // Mirror CSS auto-fill: floor((width + gap) / (minWidth + gap)).
  const cols = Math.floor((width + gap) / (minWidth + gap));
  return Math.max(1, cols);
}

const CardsView = ({
  hosts,
  selectedHost,
  revealHostId,
  revealNavigationKey,
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
  const { intentStartedAt, markIntent } = useQuickAccessIntent();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [columns, setColumns] = useState(1);
  const uiScale = useAppliedUiScale();
  const gridGap = Math.ceil(BASE_GRID_GAP * uiScale);
  const columnMinWidth = Math.ceil(BASE_COLUMN_MIN_WIDTH * uiScale);
  const cardHeight = Math.ceil(BASE_CARD_HEIGHT * uiScale);


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
  const revealHost = useCallback(() => {
    const viewport = scrollRef.current;
    if (!viewport || viewport.clientWidth <= 0 || columnsForWidth(viewport.clientWidth, columnMinWidth, gridGap) !== columns) return false;
    const index = hosts.findIndex((host) => host.id === revealHostId);
    if (index < 0) return false;
    rowVirtualizer.scrollToIndex(Math.floor(index / columns), { align: "auto" });
    return true;
  }, [columnMinWidth, columns, gridGap, hosts, revealHostId, rowVirtualizer]);
  useHostReveal(revealHostId, revealHost, { layoutKey: columns, navigationKey: revealNavigationKey });
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
              className="absolute left-0 top-0 w-full grid"
              style={{
                gap: gridGap,
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
                    onPointerEnter={() => markIntent(host.id)}
                    onFocusCapture={() => markIntent(host.id)}
                  >
                    <SurfaceCard
                      hover
                      selected={isSelected}
                      className={cn(
                        "group/card relative flex h-full flex-col overflow-hidden p-3 transition-colors duration-150 border border-border",
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
                      <SurfaceCardHeader className="mb-2 border-0 p-0 md:p-0">
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
                                <p className="min-w-0 [overflow-wrap:anywhere] text-sm font-medium text-foreground">{host.name}</p>
                                <TargetBindingWarning host={host} />
                              </div>
                              <p className="text-xs text-muted-foreground">{hostValue(host.role)}</p>
                              <p className="text-xs text-muted-foreground">
                                {hostSubtitle([
                                  host.platform,
                                  host.deviceType,
                                  host.targetKind === "device"
                                    ? t("connections.targetKind.device", "Device")
                                    : t("connections.targetKind.virtualMachine", "Virtual machine"),
                                ])}
                              </p>
                            </div>
                          </div>
                        </button>
                      </SurfaceCardHeader>
                      <SurfaceCardContent className="mb-2 flex flex-1 flex-col gap-1 p-0 text-sm md:p-0">
                        <div className="flex items-center gap-2">
                          <span className="text-xs text-muted-foreground">{t("connections.inventoryStatus")}</span>
                          <HostStatusValue status={host.status} label={inventoryResourceStatusLabel(t, host.status)} />
                        </div>
                        <div className="flex items-center gap-2">
                          <span className="text-xs text-muted-foreground">{t("connections.monitoringStatus")}</span>
                          <HostAvailabilityIndicator entry={hostLabels?.get(host.id)} />
                        </div>
                        <HostObservabilityCluster entry={hostLabels?.get(host.id)} maxLabels={2} />
                        <div className="flex items-center gap-1 text-muted-foreground">
                          <Network className="h-2.5 w-2.5" />
                          <span className="truncate font-mono text-[0.6875rem]">{hostValue(host.primaryIp)}</span>
                        </div>
                        <div className="flex min-w-0 items-center gap-1 text-muted-foreground">
                          <MapIcon className="h-2.5 w-2.5 shrink-0" />
                          <span className="min-w-0 [overflow-wrap:anywhere]">{hostSubtitle([host.region, host.site])}</span>
                        </div>
                      </SurfaceCardContent>
                      <SurfaceCardFooter className="mt-auto flex-nowrap justify-between gap-2 px-0 pb-0 pt-2">
                        <Button variant="ghost" size="sm" onClick={() => onSelectHost(host)}>
                          {t("connections.details")}
                        </Button>
                        <QuickAccessButtons
                          host={host}
                          config={quickAccessConfig}
                          onConnect={onConnect}
                          onPlaybookQuickAction={onPlaybookQuickAction}
                          variant="card"
                          intentStartedAt={intentStartedAt(host.id)}
                          onIntent={() => markIntent(host.id)}
                        />
                      </SurfaceCardFooter>
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
