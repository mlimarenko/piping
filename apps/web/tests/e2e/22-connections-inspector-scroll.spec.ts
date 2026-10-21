import { captureLayoutThemes, expect, test } from "./fixtures";

for (const viewport of [{ width: 1000, height: 450 }, { width: 390, height: 600 }]) {
  test(`selected target details use the available height and scroll at ${viewport.width}px`, async ({ appPage: page }) => {
    await page.setViewportSize(viewport);
    await page.getByRole("button", { name: /^web-prod-01/ }).first().click();
    const inspector = page.getByRole("complementary", { name: /selected target details|сведения о выбранной цели/i });
    await expect(inspector).toBeVisible();
    for (const name of [/^Info$|^Информация$/i, /^Details$|^Подробности$/i]) {
      const toggle = inspector.getByRole("button", { name });
      if (await toggle.count() && await toggle.getAttribute("aria-expanded") !== "true") await toggle.click();
    }
    const section = inspector.locator("section").first();
    const body = section.locator(":scope > .overflow-y-auto");
    const available = await inspector.evaluate(element => element.clientHeight - element.querySelector("section")!.getBoundingClientRect().top + element.getBoundingClientRect().top);
    expect((await section.boundingBox())!.height, "details must fill the remaining finite pane height").toBeGreaterThanOrEqual(available - 2);
    const paneBox = (await inspector.boundingBox())!;
    const sectionBox = (await section.boundingBox())!;
    expect(sectionBox.y + sectionBox.height, "details must stay inside the pane").toBeLessThanOrEqual(paneBox.y + paneBox.height + 1);
    await expect.poll(() => body.evaluate(element => element.scrollHeight - element.clientHeight)).toBeGreaterThan(0);
    await body.hover();
    await page.mouse.wheel(0, 700);
    await expect.poll(() => body.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
    await expect(body).toBeInViewport();
    await captureLayoutThemes(page, process.env.PWA117_PROOF_DIR, `inspector-${viewport.width}`);
    const close = section.getByRole("button", { name: /^Close$|^Закрыть$/i }).first();
    await close.scrollIntoViewIfNeeded();
    await expect(close).toBeInViewport();
  });
}

test("one occupied pane has one control row and retains its terminal through splitting", async ({ page }) => {
  const sockets: string[] = [];
  const closes: string[] = [];
  await page.routeWebSocket(/\/chrome-proof\//, socket => {
    sockets.push(socket.url());
    socket.onClose(() => closes.push(socket.url()));
    socket.onMessage(message => socket.send(message));
    socket.send("Local chrome fixture ready\r\n");
  });
  await page.addInitScript(() => {
    (window as Window & { __PIPING_DETACHED_GROUP_HANDOFF__?: unknown }).__PIPING_DETACHED_GROUP_HANDOFF__ = [{
      sessionId: "chrome-proof-1", hostId: "chrome-proof-1", hostName: "Local terminal",
      protocol: "ssh", websocketUrl: "ws://127.0.0.1:4277/chrome-proof/1", token: "fixture",
    }];
  });
  await page.setViewportSize({ width: 1000, height: 700 });
  await page.goto("/connections/detached-group?ids=chrome-proof-1");
  const workspace = page.getByTestId("session-workspace");
  const viewer = workspace.locator("[data-session-pane-viewer]");
  await expect.poll(() => sockets.length).toBe(1);
  const original = await viewer.elementHandle();
  const assertCompact = async () => {
    await expect(workspace.getByRole("combobox")).toHaveCount(0);
    const gridBox = (await workspace.locator("[data-session-pane-grid]").boundingBox())!;
    const viewerBox = (await viewer.boundingBox())!;
    expect(Math.abs(gridBox.y - viewerBox.y)).toBeLessThanOrEqual(1);
    expect(Math.abs(gridBox.height - viewerBox.height)).toBeLessThanOrEqual(1);
  };
  await assertCompact();
  await captureLayoutThemes(page, process.env.PWA117_PROOF_DIR, "session-desktop");
  await workspace.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Split pane 1 stacked", exact: true }).click();
  await expect(workspace.getByRole("combobox", { name: "Session in pane 1", exact: true })).toBeVisible();
  await workspace.getByRole("button", { name: "Pane 2 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Remove pane (keep session)", exact: true }).click();
  await assertCompact();
  expect(await viewer.evaluate((element, node) => element === node, original)).toBe(true);
  await page.setViewportSize({ width: 390, height: 600 });
  await assertCompact();
  await captureLayoutThemes(page, process.env.PWA117_PROOF_DIR, "session-mobile");
  expect(sockets).toHaveLength(1);
  expect(closes).toHaveLength(0);
});
