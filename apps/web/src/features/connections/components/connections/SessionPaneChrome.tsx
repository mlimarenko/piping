import { useId, type PointerEvent, type ReactNode } from "react";
import { Root as Separator } from "@radix-ui/react-separator";
import { useTranslation } from "react-i18next";
import { Columns2, Rows2, Maximize2, Minimize2, X, MoreHorizontal } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/shared/ui-primitives/dropdown-menu";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/shared/ui-primitives/tooltip";
import { cn } from "@/shared/lib/utils";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import { paneRectStyle, paneGeometry, paneSplitBounds, sessionPanes, splitSessionPane, placePaneSession, removeSessionPane, resizePaneSplit, MAX_SESSION_PANES, MIN_SESSION_PANE_SIZE, type PaneDirection, type PaneRect, type PaneSplit, type SessionPane, type SessionPaneLayout } from "./sessionPaneLayout";

type GridSize = { width: number; height: number } | null;

function splitBlockReason(direction: PaneDirection, paneSize: GridSize, full: boolean) {
  if (full) return "limit";
  const axis = direction === "horizontal" ? "width" : "height";
  if (paneSize && paneSize[axis] < MIN_SESSION_PANE_SIZE[axis] * 2) return "tooSmall";
  return null;
}

/** Split actions belong to the pane they affect, including the first pane. */
function SplitActions({ layout, paneSize, onChange, number, mobile, splitBlockedReason }: Readonly<{
  layout: SessionPaneLayout;
  paneSize: GridSize;
  onChange: (layout: SessionPaneLayout) => void;
  number: number;
  mobile?: boolean;
  splitBlockedReason?: "returnToGroup";
}>) {
  const { t } = useTranslation();
  const full = sessionPanes(layout.root).length >= MAX_SESSION_PANES;
  const directions = mobile ? ["vertical"] as const : ["horizontal", "vertical"] as const;
  return <>{directions.map(direction => {
    const reason = splitBlockedReason ?? splitBlockReason(direction, mobile ? null : paneSize, full);
    const titleKey = reason ?? direction;
    const name = t(`connections.panes.${direction}Pane`, { number });
    return <DropdownMenuItem key={direction} disabled={reason !== null} title={t(`connections.panes.${titleKey}`)}
      className={mobile ? "min-h-[44px]" : undefined} onSelect={() => onChange(splitSessionPane(layout, direction))}>
      {direction === "horizontal" ? <Columns2 className="size-4" aria-hidden="true" /> : <Rows2 className="size-4" aria-hidden="true" />}
      {name}
    </DropdownMenuItem>;
  })}</>;
}

function PaneSessionSelect({ node, number, layout, sessions, onChange, onFocus, className, mobile }: Readonly<{
  node: SessionPane; number: number; layout: SessionPaneLayout; sessions: LiveSession[]; mobile?: boolean;
  onChange: (layout: SessionPaneLayout) => void; onFocus: (paneId: string) => void; className?: string;
}>) {
  const { t } = useTranslation();
  return <select aria-label={t("connections.panes.choose", { number })} value={node.sessionId ?? ""}
    className={cn("min-w-0 rounded-md border border-transparent bg-transparent px-2 text-sm text-foreground hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring", mobile ? "h-11 min-h-[44px]" : "h-9", className)}
    onFocus={() => onFocus(node.id)} onChange={event => onChange(placePaneSession(layout, node.id, event.target.value || null))}>
    <option value="">{t("connections.panes.empty")}</option>
    {sessions.map(session => <option key={session.id} value={session.id}>{session.protocol.toUpperCase()} · {session.hostName}</option>)}
  </select>;
}

interface Props {
  renderHealth?: (session: LiveSession, compact?: boolean) => ReactNode;
  mobile?: boolean;
  layout: SessionPaneLayout;
  sessions: LiveSession[];
  onChange: (layout: SessionPaneLayout) => void;
  onFocus: (paneId: string) => void;
  onResize: (event: PointerEvent<HTMLElement>, split: PaneSplit, rect: PaneRect) => void;
  gridSize: GridSize;
}

function separatorKeyRatio(key: string, split: PaneSplit, min: number, max: number): number | null {
  if (key === "Home") return min;
  if (key === "End") return max;
  const negative = split.direction === "horizontal" ? "ArrowLeft" : "ArrowUp";
  const positive = split.direction === "horizontal" ? "ArrowRight" : "ArrowDown";
  if (key === negative) return Math.max(min, split.ratio - 0.05);
  if (key === positive) return Math.min(max, split.ratio + 0.05);
  return null;
}

function SplitSeparator({ node, rect, layout, gridSize, onChange, onResize }: Readonly<Pick<Props, "layout" | "gridSize" | "onChange" | "onResize"> & { node: PaneSplit; rect: PaneRect }>) {
  const { t } = useTranslation();
  const horizontal = node.direction === "horizontal";
  const { min, max } = paneSplitBounds(node, rect, gridSize);
  const position = horizontal ? rect.x + rect.width * node.ratio : rect.y + rect.height * node.ratio;
  return <Separator decorative={false} tabIndex={0} aria-label={t("connections.panes.resize")}
    orientation={horizontal ? "vertical" : "horizontal"} aria-orientation={horizontal ? "vertical" : "horizontal"} aria-valuemin={Math.round(min * 100)} aria-valuemax={Math.round(max * 100)} aria-valuenow={Math.round(node.ratio * 100)}
    className={cn("group absolute z-40 m-0 touch-none border-0 bg-transparent focus-visible:outline-none", horizontal ? "w-2 -translate-x-1/2 cursor-ew-resize" : "h-2 -translate-y-1/2 cursor-ns-resize")}
    style={horizontal ? { left: `${position}%`, top: `${rect.y}%`, height: `${rect.height}%` } : { top: `${position}%`, left: `${rect.x}%`, width: `${rect.width}%` }}
    onPointerDown={event => onResize(event, node, rect)} onKeyDown={event => {
      const ratio = separatorKeyRatio(event.key, node, min, max);
      if (ratio === null) return;
      event.preventDefault();
      event.stopPropagation();
      onChange(resizePaneSplit(layout, node.id, Math.max(min, Math.min(max, ratio))));
    }} >
      {/* Keep the resize target wide without painting over neighbouring cells. */}
      <span aria-hidden="true" className={cn("pointer-events-none absolute bg-border group-hover:bg-primary group-focus-visible:bg-primary", horizontal ? "inset-y-0 left-1/2 w-px -translate-x-1/2" : "inset-x-0 top-1/2 h-px -translate-y-1/2")} />
    </Separator>;
}

export function PaneActions({ node, number, layout, paneSize, onChange, mobile, compact, showMaximize, splitBlockedReason }: Readonly<{
  node: SessionPane; number: number; layout: SessionPaneLayout; paneSize: GridSize;
  onChange: Props["onChange"]; mobile?: boolean; compact?: boolean; showMaximize?: boolean;
  splitBlockedReason?: "returnToGroup";
}>) {
  const { t } = useTranslation();
  const hintId = useId();
  const hint = splitBlockedReason ? t(`connections.panes.${splitBlockedReason}`) : null;
  return <><span id={hintId} className="sr-only">{hint}</span><DropdownMenu>
    <TooltipProvider><Tooltip><TooltipTrigger asChild>
      <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" className={cn("shrink-0", mobile ? "size-11" : compact ? "size-7" : "size-9")} aria-label={t("connections.panes.actions", { number })} aria-describedby={hint ? hintId : undefined} title={t("connections.panes.actions", { number })}><MoreHorizontal className="size-4" aria-hidden="true" /></Button></DropdownMenuTrigger>
    </TooltipTrigger>{hint && <TooltipContent>{hint}</TooltipContent>}</Tooltip></TooltipProvider>
    <DropdownMenuContent align="end">
      {hint && <p className="max-w-64 px-2 py-1.5 text-sm text-muted-foreground">{hint}</p>}
      <SplitActions layout={{ ...layout, focusedPaneId: node.id }} paneSize={paneSize} onChange={onChange} number={number} mobile={mobile} splitBlockedReason={splitBlockedReason} />
      {showMaximize && <DropdownMenuItem onSelect={() => onChange({ ...layout, focusedPaneId: node.id, maximizedPaneId: layout.maximizedPaneId ? null : node.id })}>
        {layout.maximizedPaneId ? <Minimize2 className="size-4" aria-hidden="true" /> : <Maximize2 className="size-4" aria-hidden="true" />}
        {t(`connections.panes.${layout.maximizedPaneId ? "restore" : "maximize"}`)}
      </DropdownMenuItem>}
      {sessionPanes(layout.root).length > 1 && <>
        <DropdownMenuSeparator />
        <DropdownMenuItem className={mobile ? "min-h-[44px]" : undefined} onSelect={() => onChange(removeSessionPane(layout, node.id))}><X className="size-4" aria-hidden="true" />{t("connections.panes.remove")}</DropdownMenuItem>
      </>}
    </DropdownMenuContent>
  </DropdownMenu></>;
}

function PaneHeader({ node, rect, number, layout, sessions, gridSize, onChange, onFocus, mobile, renderHealth }: Readonly<Pick<Props, "layout" | "sessions" | "onChange" | "onFocus" | "gridSize" | "mobile" | "renderHealth"> & { node: SessionPane; rect: PaneRect; number: number }>) {
  const { t } = useTranslation();
  const effectiveRect = layout.maximizedPaneId ? { x: 0, y: 0, width: 100, height: 100 } : rect;
  const focused = layout.focusedPaneId === node.id;
  const maximizeKey = layout.maximizedPaneId ? "restore" : "maximize";
  const actionSize = cn("shrink-0", mobile ? "size-11 min-h-[44px] min-w-[44px]" : "size-9");
  const paneSize = gridSize ? { width: gridSize.width * rect.width / 100, height: gridSize.height * rect.height / 100 } : null;
  const compact = !mobile && gridSize !== null && gridSize.width * effectiveRect.width / 100 < 240;
  const session = sessions.find(item => item.id === node.sessionId);
  return <div style={paneRectStyle(effectiveRect)} className="pointer-events-none absolute z-30 min-h-0 min-w-0">
      <div onPointerDown={() => onFocus(node.id)} className={cn("pointer-events-auto flex h-[var(--session-pane-header-height,2.25rem)] min-w-0 items-center gap-1 border-b border-border pl-2 pr-1", focused ? "bg-accent shadow-[inset_3px_0_0_hsl(var(--primary))]" : "bg-muted")}>
        {node.sessionId ? <PaneSessionSelect node={node} number={number} layout={layout} sessions={sessions} onChange={onChange} onFocus={onFocus} mobile={mobile} className="flex-1" />
          : <span className="min-w-0 flex-1 truncate px-2 text-sm text-muted-foreground">{t("connections.panes.paneNumber", { number })}</span>}
        {session && renderHealth?.(session, compact)}
        {!compact && <Button variant="ghost" size="icon-sm" className={actionSize} aria-label={t(`connections.panes.${maximizeKey}`)} title={t(`connections.panes.${maximizeKey}`)}
          onClick={() => onChange({ ...layout, focusedPaneId: node.id, maximizedPaneId: layout.maximizedPaneId ? null : node.id })}>
          {layout.maximizedPaneId ? <Minimize2 className="size-4" aria-hidden="true" /> : <Maximize2 className="size-4" aria-hidden="true" />}
        </Button>}
        <PaneActions node={node} number={number} layout={layout} paneSize={paneSize} onChange={onChange} mobile={mobile} compact={compact} showMaximize={compact} />
      </div>
      {!node.sessionId && <div className="pointer-events-auto flex h-[calc(100%-var(--session-pane-header-height,2.25rem))] flex-col items-center justify-center gap-3 p-5 text-center text-sm text-muted-foreground">
        <span className="text-sm font-medium text-foreground">{t("connections.panes.empty")}</span>
        <PaneSessionSelect node={node} number={number} layout={layout} sessions={sessions} onChange={onChange} onFocus={onFocus} mobile={mobile} className="w-full max-w-72 border-input bg-card" />
        <span className="max-w-72 leading-relaxed">{t("connections.panes.emptyHint")}</span>
      </div>}
    </div>;
}

export default function SessionPaneChrome(props: Readonly<Props>) {
  const { layout } = props;
  const panes = sessionPanes(layout.root);
  return <>{paneGeometry(layout.root, undefined, props.mobile).map(({ node, rect }) => {
    if (node.kind === "split") {
      return layout.maximizedPaneId ? null : <SplitSeparator key={node.id} {...props} node={node} rect={rect} />;
    }
    if (layout.maximizedPaneId && layout.maximizedPaneId !== node.id) return null;
    return <PaneHeader key={node.id} {...props} node={node} rect={rect} number={panes.indexOf(node) + 1} />;
  })}</>;
}
