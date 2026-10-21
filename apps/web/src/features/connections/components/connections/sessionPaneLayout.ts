import type { CSSProperties } from "react";

export type PaneDirection = "horizontal" | "vertical";
export type SessionPane = { kind: "pane"; id: string; sessionId: string | null };
export type PaneSplit = { kind: "split"; id: string; direction: PaneDirection; ratio: number; first: PaneNode; second: PaneNode };
export type PaneNode = SessionPane | PaneSplit;
export interface SessionPaneLayout { root: PaneNode; focusedPaneId: string; maximizedPaneId: string | null }
export interface PaneRect { x: number; y: number; width: number; height: number }
export function paneRectStyle(rect: PaneRect): CSSProperties {
  return { left: `${rect.x}%`, top: `${rect.y}%`, width: `${rect.width}%`, height: `${rect.height}%` };
}
export const MAX_SESSION_PANES = 4;
export const MIN_SESSION_PANE_SIZE = { width: 128, height: 160 };
export const clampPaneRatio = (ratio: number) => Math.max(0.2, Math.min(0.8, Number.isFinite(ratio) ? ratio : 0.5));

export function sessionPanes(node: PaneNode): SessionPane[] {
  return node.kind === "pane" ? [node] : [...sessionPanes(node.first), ...sessionPanes(node.second)];
}

export function defaultSessionPaneLayout(sessionId: string | null = null): SessionPaneLayout {
  return { root: { kind: "pane", id: "pane-1", sessionId }, focusedPaneId: "pane-1", maximizedPaneId: null };
}

/** Replace only the session assignment; focus and split geometry belong to the current layout. */
export function replacePaneSession(layout: SessionPaneLayout, previousId: string, nextId: string): SessionPaneLayout {
  return { ...layout, root: mapNode(layout.root, node => node.kind === "pane" && node.sessionId === previousId ? { ...node, sessionId: nextId } : node) };
}

function mapNode(node: PaneNode, change: (node: PaneNode) => PaneNode): PaneNode {
  return change(node.kind === "pane" ? node : { ...node, first: mapNode(node.first, change), second: mapNode(node.second, change) });
}

export function reconcilePaneLayout(layout: SessionPaneLayout, eligibleIds: readonly string[]): SessionPaneLayout {
  const eligible = new Set(eligibleIds);
  const seen = new Set<string>();
  const root = mapNode(layout.root, (node) => {
    if (node.kind !== "pane" || node.sessionId === null) return node;
    const valid = eligible.has(node.sessionId) && !seen.has(node.sessionId);
    seen.add(node.sessionId);
    return valid ? node : { ...node, sessionId: null };
  });
  const panes = sessionPanes(root);
  return { root, focusedPaneId: panes.some(p => p.id === layout.focusedPaneId) ? layout.focusedPaneId : (panes[0]?.id ?? "pane-1"),
    maximizedPaneId: panes.some(p => p.id === layout.maximizedPaneId) ? layout.maximizedPaneId : null };
}

/** Assigning a session already in another pane swaps the two assignments. */
export function placePaneSession(layout: SessionPaneLayout, paneId: string, sessionId: string | null): SessionPaneLayout {
  const target = sessionPanes(layout.root).find(p => p.id === paneId);
  if (!target) return layout;
  const root = mapNode(layout.root, node => {
    if (node.kind !== "pane") return node;
    if (node.id === paneId) return { ...node, sessionId };
    if (sessionId !== null && node.sessionId === sessionId) return { ...node, sessionId: target.sessionId };
    return node;
  });
  return { ...layout, root, focusedPaneId: paneId };
}

export function selectPaneSession(layout: SessionPaneLayout, sessionId: string): SessionPaneLayout {
  const placed = sessionPanes(layout.root).find(p => p.sessionId === sessionId);
  return placed ? { ...layout, focusedPaneId: placed.id, maximizedPaneId: layout.maximizedPaneId ? placed.id : null }
    : layout;
}

export function splitSessionPane(layout: SessionPaneLayout, direction: PaneDirection): SessionPaneLayout {
  const panes = sessionPanes(layout.root);
  if (panes.length >= MAX_SESSION_PANES) return layout;
  const ids = new Set<string>();
  mapNode(layout.root, node => { ids.add(node.id); return node; });
  let suffix = 1;
  while (ids.has(`pane-${suffix}`) || ids.has(`split-${suffix}`)) suffix++;
  const pane: SessionPane = { kind: "pane", id: `pane-${suffix}`, sessionId: null };
  return { root: mapNode(layout.root, node => node.id === layout.focusedPaneId
    ? { kind: "split", id: `split-${suffix}`, direction, ratio: 0.5, first: node, second: pane } : node),
  focusedPaneId: pane.id, maximizedPaneId: null };
}

export function removeSessionPane(layout: SessionPaneLayout, paneId: string): SessionPaneLayout {
  if (sessionPanes(layout.root).length === 1) return layout;
  function remove(node: PaneNode): PaneNode {
    if (node.kind === "pane") return node;
    if (node.first.id === paneId) return node.second;
    if (node.second.id === paneId) return node.first;
    return { ...node, first: remove(node.first), second: remove(node.second) };
  }
  return reconcilePaneLayout({ ...layout, root: remove(layout.root), maximizedPaneId: null }, sessionPanes(layout.root).flatMap(p => p.sessionId ? [p.sessionId] : []));
}

export function resizePaneSplit(layout: SessionPaneLayout, splitId: string, ratio: number): SessionPaneLayout {
  return { ...layout, root: mapNode(layout.root, node => node.kind === "split" && node.id === splitId ? { ...node, ratio: clampPaneRatio(ratio) } : node) };
}

function minimumPaneSize(node: PaneNode): { width: number; height: number } {
  if (node.kind === "pane") return MIN_SESSION_PANE_SIZE;
  const first = minimumPaneSize(node.first);
  const second = minimumPaneSize(node.second);
  const horizontal = node.direction === "horizontal";
  return {
    width: horizontal ? Math.max(first.width / node.ratio, second.width / (1 - node.ratio)) : Math.max(first.width, second.width),
    height: horizontal ? Math.max(first.height, second.height) : Math.max(first.height / node.ratio, second.height / (1 - node.ratio)),
  };
}

/** Pointer and keyboard resizing respect every descendant, not just the two branches. */
export function paneSplitBounds(split: PaneSplit, rect: PaneRect, gridSize: { width: number; height: number } | null) {
  const horizontal = split.direction === "horizontal";
  const axis = horizontal ? "width" : "height";
  const extent = gridSize ? gridSize[axis] * rect[axis] / 100 : Infinity;
  const firstSize = minimumPaneSize(split.first)[axis];
  const secondSize = minimumPaneSize(split.second)[axis];
  const min = Math.max(0.2, firstSize / extent);
  const max = Math.min(0.8, 1 - secondSize / extent);
  if (min > max) {
    const ratio = clampPaneRatio(firstSize / (firstSize + secondSize));
    return { extent, min: ratio, max: ratio };
  }
  return { extent, min, max };
}

export function paneGeometry(root: PaneNode, rect?: PaneRect, stacked = false): { node: PaneNode; rect: PaneRect }[] {
  rect ??= { x: 0, y: 0, width: 100, height: 100 };
  // Phone presentation is a projection, never a rewrite of saved desktop splits.
  if (stacked) {
    const panes = sessionPanes(root);
    const height = rect.height / panes.length;
    return panes.map((node, index) => ({ node, rect: { ...rect, y: rect.y + index * height, height } }));
  }
  if (root.kind === "pane") return [{ node: root, rect }];
  const horizontal = root.direction === "horizontal";
  const first = { ...rect, width: horizontal ? rect.width * root.ratio : rect.width, height: horizontal ? rect.height : rect.height * root.ratio };
  const second = { x: horizontal ? rect.x + first.width : rect.x, y: horizontal ? rect.y : rect.y + first.height,
    width: horizontal ? rect.width - first.width : rect.width, height: horizontal ? rect.height : rect.height - first.height };
  return [{ node: root, rect }, ...paneGeometry(root.first, first), ...paneGeometry(root.second, second)];
}

/** Strict allowlist: layout preferences can never carry handoff credentials. */
function isLayoutRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function hasOnlyLayoutKeys(value: Record<string, unknown>, allowed: string[]) {
  return Object.keys(value).every(key => allowed.includes(key));
}

function isPaneSessionId(value: unknown): value is string | null {
  if (value === null) return true;
  if (typeof value !== "string") return false;
  return /^[a-zA-Z0-9_-]{1,128}$/.test(value) || /^cctv:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(value);
}

function isSplitMetadata(value: Record<string, unknown>): value is Record<string, unknown> & { direction: PaneDirection; ratio: number } {
  return value.kind === "split" && hasOnlyLayoutKeys(value, ["kind", "id", "direction", "ratio", "first", "second"])
    && (value.direction === "horizontal" || value.direction === "vertical")
    && typeof value.ratio === "number" && Number.isFinite(value.ratio) && value.ratio >= 0.2 && value.ratio <= 0.8;
}

export function normalizeSessionPaneLayout(raw: unknown): SessionPaneLayout | null {
  const ids = new Set<string>();
  let leaves = 0;
  function parse(value: unknown, depth: number): PaneNode | null {
    if (!isLayoutRecord(value) || depth > 3) return null;
    if (typeof value.id !== "string" || !/^(pane|split)-[a-zA-Z0-9-]{1,64}$/.test(value.id) || ids.has(value.id)) return null;
    ids.add(value.id);
    if (value.kind === "pane") {
      if (!hasOnlyLayoutKeys(value, ["kind", "id", "sessionId"]) || ++leaves > MAX_SESSION_PANES || !isPaneSessionId(value.sessionId)) return null;
      return { kind: "pane", id: value.id, sessionId: value.sessionId };
    }
    if (!isSplitMetadata(value)) return null;
    const first = parse(value.first, depth + 1), second = parse(value.second, depth + 1);
    return first && second ? { kind: "split", id: value.id, direction: value.direction, ratio: value.ratio, first, second } : null;
  }
  if (!isLayoutRecord(raw) || !hasOnlyLayoutKeys(raw, ["root", "focusedPaneId", "maximizedPaneId"])) return null;
  const root = parse(raw.root, 0);
  if (!root) return null;
  const panes = sessionPanes(root);
  if (!panes.some(p => p.id === raw.focusedPaneId) || !(raw.maximizedPaneId === null || panes.some(p => p.id === raw.maximizedPaneId))) return null;
  return { root, focusedPaneId: raw.focusedPaneId as string, maximizedPaneId: raw.maximizedPaneId as string | null };
}
