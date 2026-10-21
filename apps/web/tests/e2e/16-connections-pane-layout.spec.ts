import { mkdir } from "node:fs/promises";
import { type Locator, type Page } from "@playwright/test";
import { expect, test } from "./fixtures";

const PROOF_DIR = process.env.PWA97_PROOF_DIR;
const ids = ["pane-proof-1", "pane-proof-2", "pane-proof-3"] as const;

async function screenshot(page: Page, name: string) {
  if (!PROOF_DIR) return;
  await mkdir(PROOF_DIR, { recursive: true });
  await page.screenshot({ path: `${PROOF_DIR}/${name}.png`, fullPage: true });
}

async function box(locator: Locator) {
  const rect = await locator.boundingBox();
  expect(rect, "rendered element must have native browser geometry").not.toBeNull();
  return rect!;
}

// Exercise the real routed ConnectionsPanel, not a second test renderer. The
// existing detached-group handoff admits safe synthetic SSH sessions; Playwright
// answers their sockets locally, so this fixture cannot reach a remote target.
async function settle(workspace: Locator) {
  await workspace.evaluate(async node => {
    // A cancelled native animation is terminal too; its finished promise
    // rejects with AbortError. Geometry is checked after all captured promises settle.
    await Promise.allSettled(node.getAnimations({ subtree: true }).map(animation => animation.finished));
  });
}

async function openThreePanes(page: Page, viewport = { width: 1280, height: 900 }, touch = false, sessionIds: readonly string[] = ids) {
  const sockets: string[] = [];
  const closes: string[] = [];
  const messages = new Map<string, number>();
  await page.routeWebSocket(/\/pane-proof\//, socket => {
    sockets.push(socket.url());
    socket.onMessage(message => {
      // Count the actual terminal keystroke, not resize/keepalive frames that
      // could otherwise satisfy continuity while keyboard transport is broken.
      if (message === "a") messages.set(socket.url(), (messages.get(socket.url()) ?? 0) + 1);
      socket.send(message);
    });
    socket.onClose(() => closes.push(socket.url()));
    socket.send("\r\nLocal pane fixture ready\r\n");
  });
  await page.addInitScript(sessionIds => {
    (window as Window & { __PIPING_DETACHED_GROUP_HANDOFF__?: unknown }).__PIPING_DETACHED_GROUP_HANDOFF__ = sessionIds.map((id, index) => ({
      sessionId: id, hostId: id, hostName: `Local terminal ${index + 1}`,
      protocol: "ssh", websocketUrl: `ws://127.0.0.1:4173/pane-proof/${id}`,
      token: "fixture",
    }));
  }, sessionIds);
  await page.setViewportSize(viewport);
  await page.goto(`/connections/detached-group?ids=${sessionIds.join(",")}`);
  const workspace = page.getByTestId("session-workspace");
  await expect(workspace).toBeVisible();
  await expect.poll(() => sockets.length).toBe(sessionIds.length);
    await workspace.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
    await page.getByRole("menuitem", { name: touch ? "Split pane 1 stacked" : "Split pane 1 side-by-side", exact: true }).click();
  await workspace.getByRole("combobox", { name: "Session in pane 2", exact: true }).selectOption(ids[1]);
    await workspace.getByRole("button", { name: "Pane 2 actions", exact: true }).click();
    await page.getByRole("menuitem", { name: "Split pane 2 stacked", exact: true }).click();
  await workspace.getByRole("combobox", { name: "Session in pane 3", exact: true }).selectOption(ids[2]);
  await expect(workspace.getByRole("combobox")).toHaveCount(3);
  await expect(workspace.locator("[data-session-pane-viewer]:visible")).toHaveCount(3);
  // The workspace slides in on desktop. Capture its settled geometry rather
  // than comparing an entrance-animation frame with the restored desktop.
  await settle(workspace);
  const typeIntoFirstViewer = async () => {
      await workspace.getByRole("combobox", { name: "Session in pane 1", exact: true }).focus();
    const previous = [...messages.values()].reduce((sum, count) => sum + count, 0);
    await workspace.locator(`[data-session-pane-viewer="${ids[0]}"] .xterm-helper-textarea`).press("a");
    await expect.poll(() => [...messages.values()].reduce((sum, count) => sum + count, 0)).toBeGreaterThan(previous);
  };
  return { workspace, sockets, closes, typeIntoFirstViewer };
}

test.describe("ConnectionsPanel native pane geometry", () => {
  test("desktop physical dividers expose explicit orientation", async ({ page }) => {
    const { workspace } = await openThreePanes(page);
    await screenshot(page, "desktop");
    const separators = workspace.getByRole("separator", { name: "Resize session panes" });
    await expect(separators).toHaveCount(2);
    for (const separator of await separators.all()) {
      const rect = await box(separator);
      const physicalOrientation = rect.width > rect.height ? "horizontal" : "vertical";
      expect.soft(await separator.getAttribute("aria-orientation"),
        `physical ${physicalOrientation} divider ${rect.width}x${rect.height}`).toBe(physicalOrientation);
    }
  });

  test("phone stacks all panes with touch headers and preserves desktop viewers", async ({ page }) => {
    const { workspace, sockets, closes, typeIntoFirstViewer } = await openThreePanes(page);
    await typeIntoFirstViewer();
    const viewers = ids.map(id => workspace.locator(`[data-session-pane-viewer="${id}"]`));
    const desktop = await Promise.all(viewers.map(box));
    expect(desktop[1]!.x).toBeGreaterThan(desktop[0]!.x);
    expect(desktop[2]!.y).toBeGreaterThan(desktop[1]!.y);
    const originalNodes = await Promise.all(viewers.map(viewer => viewer.elementHandle()));
    await page.setViewportSize({ width: 390, height: 844 });
    await expect.poll(async () => (await box(workspace)).width).toBe(390);
    await typeIntoFirstViewer();
    await screenshot(page, "phone");
    const grid = await box(workspace.locator("[data-session-pane-grid]"));
    const phone = await Promise.all(viewers.map(box));
    for (const [index, rect] of phone.entries()) {
      expect.soft(rect.width, `pane ${index + 1} full grid width`).toBeGreaterThanOrEqual(grid.width - 1);
      expect.soft(Math.abs(rect.x - grid.x), `pane ${index + 1} aligned to grid`).toBeLessThanOrEqual(1);
      expect.soft(rect.height, `pane ${index + 1} viewer minimum`).toBeGreaterThanOrEqual(160);
      if (index > 0) expect.soft(rect.y, `pane ${index + 1} stacked below predecessor`)
        .toBeGreaterThanOrEqual(phone[index - 1]!.y + phone[index - 1]!.height - 1);
    }
    await expect.soft(workspace.getByRole("separator", { name: "Resize session panes" })).toHaveCount(0);
    const selects = workspace.getByRole("combobox", { name: /Session in pane/ });
    await expect(selects).toHaveCount(3);
    for (const select of await selects.all()) {
      const controls = select.locator("..").locator("button, select");
      await expect(controls).toHaveCount(4);
      for (const control of await controls.all()) {
        const rect = await box(control);
        expect.soft(rect.width, `${await control.getAttribute("aria-label")} touch width`).toBeGreaterThanOrEqual(44);
        expect.soft(rect.height, `${await control.getAttribute("aria-label")} touch height`).toBeGreaterThanOrEqual(44);
      }
    }
    await page.setViewportSize({ width: 1280, height: 900 });
    await expect.poll(async () => (await box(workspace)).width).toBe(1280);
    await settle(workspace);
    await typeIntoFirstViewer();
    await screenshot(page, "desktop-restored");
    for (const [index, viewer] of viewers.entries()) {
      const restored = await box(viewer);
      for (const axis of ["x", "y", "width", "height"] as const)
        expect.soft(Math.abs(restored[axis] - desktop[index]![axis]), `restored pane ${index + 1} ${axis}`).toBeLessThanOrEqual(1);
      expect.soft(await viewer.evaluate((node, original) => node === original, originalNodes[index])).toBe(true);
    }
    expect(sockets, "breakpoint changes must not open replacement sockets").toHaveLength(3);
    expect(closes, "breakpoint changes must not close viewer sockets").toHaveLength(0);
  });

  test("cold phone start renders three full-width panes and maximize/restore", async ({ page }) => {
    const { workspace, typeIntoFirstViewer } = await openThreePanes(page, { width: 390, height: 844 }, true);
    const grid = await box(workspace.locator("[data-session-pane-grid]"));
    for (const viewer of await workspace.locator("[data-session-pane-viewer]").all()) {
      const rect = await box(viewer);
      expect(rect.width).toBeGreaterThanOrEqual(grid.width - 1);
      expect(rect.height).toBeGreaterThanOrEqual(160);
    }
    await typeIntoFirstViewer();
    await screenshot(page, "cold-phone");
    await workspace.getByRole("button", { name: "Maximize pane", exact: true }).first().click();
    await expect(workspace.locator("[data-session-pane-viewer]:visible")).toHaveCount(1);
    const maximized = await box(workspace.locator(`[data-session-pane-viewer="${ids[0]}"]`));
    expect(maximized.height).toBeGreaterThan(500);
    await workspace.getByRole("button", { name: /Restore/, exact: false }).last().click();
    await expect(workspace.locator("[data-session-pane-viewer]:visible")).toHaveCount(3);
    await screenshot(page, "cold-phone-restored");
  });

  test("cancelled native animations finish settling before pane geometry checks", async ({ page }) => {
    const { workspace, typeIntoFirstViewer } = await openThreePanes(page, { width: 390, height: 844 }, true);
    await workspace.evaluate(node => {
      const animation = node.animate([{ opacity: 1 }, { opacity: 1 }], { duration: 10_000 });
      const getAnimations = node.getAnimations.bind(node);
      // Cancel after settle captures the native finished promise, not before
      // it discovers the animation. The microtask makes that boundary exact.
      node.getAnimations = options => {
        node.getAnimations = getAnimations;
        const animations = getAnimations(options);
        queueMicrotask(() => animation.cancel());
        return animations;
      };
    });
    await settle(workspace);
    expect(await workspace.evaluate(node => node.getAnimations({ subtree: true }).length)).toBe(0);
    const grid = await box(workspace.locator("[data-session-pane-grid]"));
    expect((await box(workspace)).width).toBe(390);
    for (const viewer of await workspace.locator("[data-session-pane-viewer]").all()) {
      const rect = await box(viewer);
      expect(rect.width).toBeGreaterThanOrEqual(grid.width - 1);
      expect(rect.height).toBeGreaterThanOrEqual(160);
    }
    await typeIntoFirstViewer();
  });

  test("coarse-pointer landscape stacks four panes with scrollable minimum viewers", async ({ browser }) => {
    const context = await browser.newContext({ hasTouch: true, baseURL: test.info().project.use.baseURL });
    const page = await context.newPage();
    try {
      const fourthId = "pane-proof-4";
      const { workspace, sockets, closes, typeIntoFirstViewer } = await openThreePanes(page, { width: 1280, height: 1400 }, true, [...ids, fourthId]);
      await workspace.getByRole("button", { name: "Pane 3 actions", exact: true }).click();
      await page.getByRole("menuitem", { name: "Split pane 3 stacked", exact: true }).click();
      await expect(workspace.getByRole("combobox")).toHaveCount(4);
      await workspace.getByRole("combobox", { name: "Session in pane 4", exact: true }).selectOption(fourthId);
      await page.setViewportSize({ width: 844, height: 390 });
      await expect.poll(async () => (await box(workspace)).width).toBe(844);
      expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
      await typeIntoFirstViewer();
      const grid = workspace.locator("[data-session-pane-grid]");
      const gridBox = await box(grid);
      for (const viewer of await workspace.locator("[data-session-pane-viewer]").all()) {
        const rect = await box(viewer);
        expect(rect.width).toBeGreaterThanOrEqual(gridBox.width - 1);
        expect(rect.height).toBeGreaterThanOrEqual(160);
      }
      await expect(workspace.getByRole("separator", { name: "Resize session panes" })).toHaveCount(0);
      const scrollHost = grid.locator("..");
      expect(await scrollHost.evaluate(node => node.scrollHeight > node.clientHeight)).toBe(true);
      await scrollHost.evaluate(node => { node.scrollTop = node.scrollHeight; });
      expect(await scrollHost.evaluate(node => node.scrollTop)).toBeGreaterThan(0);
      await expect(workspace.getByRole("combobox", { name: "Session in pane 4", exact: true })).toBeInViewport();
      await screenshot(page, "coarse-landscape-four-scrolled");
      expect(sockets).toHaveLength(4);
      expect(closes).toHaveLength(0);
    } finally {
      await context.close();
    }
  });
});
