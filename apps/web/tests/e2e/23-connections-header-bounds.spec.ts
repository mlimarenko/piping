import { captureLayoutThemes, expect, test } from "./fixtures";

test("desktop pane controls fit the supported 128px pane width", async ({ page }) => {
  await page.routeWebSocket(/\/header-proof\//, socket => {
    socket.onMessage(message => socket.send(message));
    socket.send("Header fixture ready\r\n");
  });
  await page.addInitScript(() => {
    (window as Window & { __PIPING_DETACHED_GROUP_HANDOFF__?: unknown }).__PIPING_DETACHED_GROUP_HANDOFF__ = [{
      sessionId: "header-proof-1", hostId: "header-proof-1", hostName: "Local terminal",
      protocol: "ssh", websocketUrl: "ws://127.0.0.1:4277/header-proof/1", token: "fixture",
    }];
  });
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto("/connections/detached-group?ids=header-proof-1");
  const workspace = page.getByTestId("session-workspace");
  await workspace.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true }).click();
  await workspace.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true }).click();
  await workspace.getByRole("separator", { name: "Resize session panes" }).last().press("Home");
  const viewer = workspace.locator("[data-session-pane-viewer]");
  await expect.poll(async () => Math.round((await viewer.boundingBox())!.width)).toBe(128);
  const pane = (await viewer.boundingBox())!;
  const selector = workspace.getByRole("combobox", { name: "Session in pane 1", exact: true });
  const header = selector.locator("..");
  await expect(header.getByTestId("session-health-bar")).toBeVisible();
  const controls = header.locator("button, select");
  for (const control of await controls.all()) {
    const box = (await control.boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(pane.x);
    expect(box.x + box.width, await control.getAttribute("aria-label") ?? "control right edge").toBeLessThanOrEqual(pane.x + pane.width + 1);
  }
  expect((await selector.boundingBox())!.width, "session selector remains usable").toBeGreaterThanOrEqual(40);
  await captureLayoutThemes(page, process.env.PWA117_PROOF_DIR, "session-min-width");
  await header.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Maximize pane", exact: true }).click();
  await expect(workspace.getByRole("button", { name: "Restore panes", exact: true })).toBeVisible();
});

for (const view of ["cards", "tree"] as const) {
test(`long target identity cannot starve ${view} inspector content in a short viewport`, async ({ page }) => {
  const longName = "long-target-".repeat(20);
  await page.addInitScript(name => {
    const wrap = (original: typeof window.fetch): typeof window.fetch => async (...args) => {
      const response = await original(...args);
      const input = args[0];
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url, location.href);
      if (url.pathname !== "/api/v1/connections/targets" || !response.ok) return response;
      const data = await response.clone().json();
      data.items[0].name = name;
      return new Response(JSON.stringify(data), { status: response.status, headers: response.headers });
    };
    // MSW installs its fetch interceptor during boot; preserve this fixture at
    // the API response boundary before and after that installation.
    let current = wrap(window.fetch);
    Object.defineProperty(window, "fetch", { configurable: true, get: () => current, set: value => { current = wrap(value); } });
  }, longName);
  await page.setViewportSize({ width: 390, height: 300 });
  await page.goto("/connections");
  if (view === "tree") {
    await page.getByRole("button", { name: "Catalogue view and actions", exact: true }).click();
    await page.getByRole("menuitem", { name: "Tree", exact: true }).click();
  }
  await page.getByRole("button", { name: new RegExp(longName) }).first().click();
  const inspector = page.getByRole("complementary", { name: /selected target details/i });
  await expect(inspector).toBeVisible();
  const section = inspector.locator("section").first();
  const body = section.locator(":scope > .overflow-y-auto");
  expect((await body.boundingBox())!.height, "details retain a usable scrolling viewport").toBeGreaterThanOrEqual(100);
  const contentAction = inspector.getByRole("button", { name: /^Info$/ });
  await contentAction.scrollIntoViewIfNeeded();
  await expect(contentAction).toBeInViewport();
  await contentAction.click();
  await expect(contentAction).toHaveAttribute("aria-expanded", "true");
  await expect.poll(() => body.evaluate(element => element.scrollHeight - element.clientHeight)).toBeGreaterThan(0);
  await expect.poll(() => body.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  await captureLayoutThemes(page, process.env.PWA117_PROOF_DIR, `inspector-long-identity-${view}`);
  const close = section.getByRole("button", { name: "Close", exact: true });
  await close.scrollIntoViewIfNeeded();
  await expect(close).toBeInViewport();
});
}
