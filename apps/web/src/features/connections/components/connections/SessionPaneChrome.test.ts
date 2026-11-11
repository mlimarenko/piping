import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { createElement } from "react";
import "@/shared/i18n";
import SessionPaneChrome from "./SessionPaneChrome";
import { defaultSessionPaneLayout, paneGeometry, resizePaneSplit, splitSessionPane, type SessionPaneLayout } from "./sessionPaneLayout";

describe("nested pane resize bounds", () => {
  it.each(["horizontal", "vertical"] as const)("keeps the %s splitter focusable with keyboard and pointer controls", direction => {
    const layout = splitSessionPane(defaultSessionPaneLayout("ssh-1"), direction);
    const onChange = vi.fn();
    const onResize = vi.fn();
    render(createElement(SessionPaneChrome, {
      layout, sessions: [], gridSize: { width: 1000, height: 1000 },
      onChange, onResize, onFocus: () => {},
    }));
    const separator = screen.getByRole("separator", { name: "Resize session panes" });
    expect(separator).toHaveAttribute("tabindex", "0");
    expect(separator.getAttribute("aria-orientation") ?? "horizontal").toBe(direction === "horizontal" ? "vertical" : "horizontal");
    expect(separator).toHaveAttribute("aria-valuemin", "20");
    expect(separator).toHaveAttribute("aria-valuemax", "80");
    expect(separator).toHaveAttribute("aria-valuenow", "50");
    separator.focus();
    expect(separator).toHaveFocus();
    fireEvent.keyDown(separator, { key: direction === "horizontal" ? "ArrowRight" : "ArrowDown" });
    expect(onChange).toHaveBeenLastCalledWith(resizePaneSplit(layout, layout.root.id, 0.55));
    fireEvent.pointerDown(separator, { pointerId: 1 });
    expect(onResize).toHaveBeenCalledExactlyOnceWith(expect.anything(), layout.root, { x: 0, y: 0, width: 100, height: 100 });
  });

  it("keeps both children of a nested branch usable when its parent separator moves", () => {
    let layout = splitSessionPane(defaultSessionPaneLayout("ssh-1"), "horizontal");
    layout = splitSessionPane({ ...layout, focusedPaneId: "pane-1" }, "horizontal");
    let changed: SessionPaneLayout | undefined;
    render(createElement(SessionPaneChrome, {
      layout, sessions: [], gridSize: { width: 640, height: 800 },
      onChange: (next: SessionPaneLayout) => { changed = next; },
      onFocus: () => {}, onResize: () => {},
    }));
    fireEvent.keyDown(screen.getAllByRole("separator")[0]!, { key: "Home" });
    expect(changed).toBeDefined();
    for (const { node, rect } of paneGeometry(changed!.root)) {
      if (node.kind === "pane") expect(rect.width * 640 / 100).toBeGreaterThanOrEqual(128);
    }
  });

  it("accounts for unequal descendant ratios in ancestor keyboard bounds", () => {
    let layout = splitSessionPane(defaultSessionPaneLayout("ssh-1"), "horizontal");
    layout = splitSessionPane({ ...layout, focusedPaneId: "pane-1" }, "horizontal");
    layout = resizePaneSplit(layout, "split-3", 0.25);
    let changed: SessionPaneLayout | undefined;
    render(createElement(SessionPaneChrome, {
      layout, sessions: [], gridSize: { width: 1024, height: 800 },
      onChange: (next: SessionPaneLayout) => { changed = next; },
      onFocus: () => {}, onResize: () => {},
    }));
    fireEvent.keyDown(screen.getAllByRole("separator")[0]!, { key: "Home" });
    expect(changed).toBeDefined();
    for (const { node, rect } of paneGeometry(changed!.root)) {
      if (node.kind === "pane") expect(rect.width * 1024 / 100).toBeGreaterThanOrEqual(128);
    }
  });
});
