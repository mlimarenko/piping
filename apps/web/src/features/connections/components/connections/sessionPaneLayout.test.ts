import { describe, expect, it } from "vitest";
import { defaultSessionPaneLayout, normalizeSessionPaneLayout, paneGeometry, placePaneSession, reconcilePaneLayout, removeSessionPane, replacePaneSession, resizePaneSplit, selectPaneSession, sessionPanes, splitSessionPane } from "./sessionPaneLayout";

describe("session pane layout", () => {
  it("replaces only the failed session assignment using the current geometry and focus", () => {
    const split = resizePaneSplit(splitSessionPane(defaultSessionPaneLayout("old-a"), "horizontal"), "split-2", 0.37);
    const layout = { ...placePaneSession(split, split.focusedPaneId, "live-b"), maximizedPaneId: split.focusedPaneId };
    const next = replacePaneSession(layout, "old-a", "new-a");
    expect(paneGeometry(next.root).map(item => item.rect)).toEqual(paneGeometry(layout.root).map(item => item.rect));
    expect(sessionPanes(next.root).map(item => [item.id, item.sessionId])).toEqual([["pane-1", "new-a"], ["pane-2", "live-b"]]);
    expect(next.focusedPaneId).toBe(layout.focusedPaneId);
    expect(next.maximizedPaneId).toBe(layout.maximizedPaneId);
    expect(replacePaneSession(layout, "closed-a", "new-a")).toEqual(layout);
    expect(sessionPanes(layout.root)[0]?.sessionId).toBe("old-a");
  });
  it("splits both directions and nests four distinct panes with a hard limit", () => {
    let layout = splitSessionPane(defaultSessionPaneLayout("ssh"), "horizontal");
    expect(paneGeometry(layout.root).filter(p => p.node.kind === "pane").map(p => p.rect.width)).toEqual([50, 50]);
    layout = splitSessionPane(layout, "vertical");
    layout = splitSessionPane(layout, "horizontal");
    expect(sessionPanes(layout.root)).toHaveLength(4);
    expect(splitSessionPane(layout, "vertical")).toBe(layout);
    expect(normalizeSessionPaneLayout(layout)).toEqual(layout);
  });

  it("swaps occupied panes rather than duplicating a session and focuses existing placement", () => {
    const split = splitSessionPane(defaultSessionPaneLayout("ssh"), "vertical");
    const placed = placePaneSession(split, split.focusedPaneId, "rdp");
    const swapped = placePaneSession(placed, placed.focusedPaneId, "ssh");
    expect(sessionPanes(swapped.root).map(p => p.sessionId)).toEqual(["rdp", "ssh"]);
    expect(selectPaneSession(swapped, "rdp").focusedPaneId).toBe("pane-1");
    expect(selectPaneSession({ ...swapped, maximizedPaneId: swapped.focusedPaneId }, "rdp").maximizedPaneId).toBe("pane-1");
  });

  it("presents every leaf full-width on phones without changing the desktop tree", () => {
    let layout = resizePaneSplit(splitSessionPane(defaultSessionPaneLayout("ssh"), "horizontal"), "split-2", 0.6);
    layout = splitSessionPane(layout, "vertical");
    layout = splitSessionPane(layout, "horizontal");
    const before = structuredClone(layout);
    const desktop = paneGeometry(layout.root);
    const stacked = paneGeometry(layout.root, undefined, true);
    expect(stacked.map(item => item.node)).toEqual(sessionPanes(layout.root));
    expect(stacked.map(item => item.rect)).toEqual([
      { x: 0, y: 0, width: 100, height: 25 },
      { x: 0, y: 25, width: 100, height: 25 },
      { x: 0, y: 50, width: 100, height: 25 },
      { x: 0, y: 75, width: 100, height: 25 },
    ]);
    expect(layout).toEqual(before);
    expect(paneGeometry(layout.root)).toEqual(desktop);
    expect(paneGeometry(layout.root, { x: 10, y: 20, width: 70, height: 60 }, true).map(item => item.rect)).toEqual([
      { x: 10, y: 20, width: 70, height: 15 },
      { x: 10, y: 35, width: 70, height: 15 },
      { x: 10, y: 50, width: 70, height: 15 },
      { x: 10, y: 65, width: 70, height: 15 },
    ]);
  });

  it("clamps resize and does not alter session identity", () => {
    const split = splitSessionPane(defaultSessionPaneLayout("ssh"), "horizontal");
    expect(resizePaneSplit(split, split.root.id, -1).root).toMatchObject({ ratio: 0.2 });
    expect(resizePaneSplit(split, split.root.id, Infinity).root).toMatchObject({ ratio: 0.5 });
    expect(resizePaneSplit(split, split.root.id, 2).root).toMatchObject({ ratio: 0.8 });
    expect(sessionPanes(resizePaneSplit(split, split.root.id, 0.7).root)[0]?.sessionId).toBe("ssh");
  });

  it("round-trips canonical CCTV IDs alongside proxied session identities", () => {
    const layout = placePaneSession(splitSessionPane(defaultSessionPaneLayout("01a0fc11-0bca-73b0-b52e-545fa666167e"), "vertical"), "pane-2", "cctv:00000000-0000-7000-8000-000000000005");
    expect(normalizeSessionPaneLayout(layout)).toEqual(layout);
    expect(normalizeSessionPaneLayout(defaultSessionPaneLayout("cctv:not-a-host-id"))).toBeNull();
  });

  it("removing a pane never removes a live session from the caller's available list", () => {
    const available = ["ssh", "rdp", "vnc"];
    const split = splitSessionPane(defaultSessionPaneLayout("ssh"), "vertical");
    const placed = placePaneSession(split, split.focusedPaneId, "rdp");
    const removed = removeSessionPane({ ...placed, maximizedPaneId: placed.focusedPaneId }, placed.focusedPaneId);
    expect(sessionPanes(removed.root).map(p => p.sessionId)).toEqual(["ssh"]);
    expect(removed.maximizedPaneId).toBeNull();
    expect(available).toEqual(["ssh", "rdp", "vnc"]);
  });

  it("reconciles multi-session close/detach and stale restored identities without reviving them", () => {
    const split = splitSessionPane(defaultSessionPaneLayout("ssh"), "vertical");
    const placed = placePaneSession(split, split.focusedPaneId, "rdp");
    const cleared = reconcilePaneLayout(placed, ["vnc"]);
    expect(sessionPanes(cleared.root).map(p => p.sessionId)).toEqual([null, null]);
    expect(sessionPanes(reconcilePaneLayout(cleared, ["ssh", "rdp"]).root).map(p => p.sessionId)).toEqual([null, null]);
    expect(selectPaneSession(cleared, "vnc")).toBe(cleared);
  });

  it.each([
    { ...defaultSessionPaneLayout(), token: "rejected-fixture" },
    { ...defaultSessionPaneLayout(), root: { ...defaultSessionPaneLayout().root, websocketUrl: "ws://invalid" } },
    { ...defaultSessionPaneLayout(), focusedPaneId: "unknown" },
    { ...defaultSessionPaneLayout(), maximizedPaneId: "unknown" },
    { ...defaultSessionPaneLayout(), root: { kind: "pane", id: "bad", sessionId: null } },
    { ...defaultSessionPaneLayout(), root: { kind: "pane", id: "pane-1", sessionId: "contains?token=secret" } },
    { ...splitSessionPane(defaultSessionPaneLayout(), "vertical"), root: { ...splitSessionPane(defaultSessionPaneLayout(), "vertical").root, ratio: NaN } },
    { ...splitSessionPane(defaultSessionPaneLayout(), "vertical"), root: { ...splitSessionPane(defaultSessionPaneLayout(), "vertical").root, ratio: 0.99 } },
  ])("rejects tampered metadata %#", raw => {
    expect(normalizeSessionPaneLayout(raw)).toBeNull();
  });
});
