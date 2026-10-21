import { apiRequest, captureLayoutThemes, expect, test, waitForConnectionsPageReady } from "./fixtures";

test("manual sidebar choice survives retained Connections and route navigation", async ({ page }) => {
  test.setTimeout(90_000);
  const opens: string[] = [];
  const closes: string[] = [];
  const inputs: string[] = [];
  await page.addInitScript(() => {
    const fetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const request = new Request(input instanceof Request ? input.clone() : new URL(String(input), location.href), init);
      const response = await fetch(input, init);
      if (request.method !== "POST" || new URL(request.url).pathname !== "/api/v1/connections/sessions" || !response.ok) return response;
      const body = await response.json();
      body.proxy.websocket_url = `ws://${location.host}/sidebar-proof/${body.session.id}`;
      return new Response(JSON.stringify(body), { status: response.status, headers: response.headers });
    };
  });
  await page.routeWebSocket(/\/sidebar-proof\//, socket => {
    const id = new URL(socket.url()).pathname;
    opens.push(id);
    socket.onClose(() => closes.push(id));
    socket.onMessage(message => { if (message === "a") inputs.push(id); });
    socket.send("\r\nSidebar retained terminal\r\n");
  });
  await page.setViewportSize({ width: 960, height: 900 });
  await page.goto("/connections");
  await waitForConnectionsPageReady(page);
  await page.getByTestId(/^quick-access-/).first().click();
  await page.getByRole("menuitem").filter({ hasText: /SSH/ }).first().click();
  const workspace = page.getByTestId("session-workspace");
  await expect(workspace).toBeVisible();
  const right = workspace.getByRole("button", { name: /dock.*right|справа/i });
  if (await right.count()) await right.first().click();
  const viewer = workspace.locator(".xterm").first();
  await expect(viewer.locator(".xterm-rows")).toContainText("Sidebar retained terminal");
  const original = await viewer.elementHandle();
  const sidebar = page.locator('[data-slot="app-sidebar"]');
  const toggle = sidebar.getByRole("button", { name: /toggle sidebar/i });
  const width = async () => Math.round((await sidebar.boundingBox())!.width);
  // Both presses must change real rendered geometry even with a live right panel.
  await toggle.click();
  await expect.poll(width).toBe(56);
  await toggle.click();
  await expect.poll(width).toBe(256);
  await captureLayoutThemes(page, process.env.PWA156_PROOF_DIR, "connections-960-expanded");
  for (const size of [768, 960, 1440]) {
    await page.setViewportSize({ width: size, height: 900 });
    await expect.poll(width).toBe(256);
    await expect.poll(async () => {
      const panel = (await workspace.boundingBox())!;
      const search = (await page.getByPlaceholder(/search targets/i).boundingBox())!;
      return size === 768 ? panel.y > search.y + search.height : panel.x > search.x + search.width;
    }).toBe(true);
    const main = (await page.locator("main").boundingBox())!;
    const panel = (await workspace.boundingBox())!;
    const search = (await page.getByPlaceholder(/search targets/i).boundingBox())!;
    expect(search.width, `search remains usable at ${size}`).toBeGreaterThanOrEqual(140);
    expect(panel.x + panel.width, `panel right edge at ${size}`).toBeLessThanOrEqual(main.x + main.width + 1);
    expect(search.x + search.width, `catalogue right edge at ${size}`).toBeLessThanOrEqual(main.x + main.width + 1);
    if (size === 768) expect(panel.y).toBeGreaterThan(search.y + search.height);
    else {
      expect(panel.width).toBeGreaterThanOrEqual(320);
      expect(panel.x).toBeGreaterThan(search.x + search.width);
    }
    for (const control of [page.getByRole("button", { name: "Filters", exact: true }),
      page.getByRole("button", { name: "Catalogue view and actions", exact: true }),
      workspace.getByRole("button", { name: "Pane 1 actions", exact: true })]) {
      await expect(control).toBeVisible();
      const box = (await control.boundingBox())!;
      expect(box.x).toBeGreaterThanOrEqual(main.x);
      expect(box.x + box.width).toBeLessThanOrEqual(main.x + main.width + 1);
    }
    expect(await viewer.evaluate((element, node) => element === node, original)).toBe(true);
    const preference = await apiRequest(page, "GET", "/api/v1/settings/effective/connections.workspace.panel");
    expect((await preference.json() as { value: { dockPosition: string } }).value.dockPosition).toBe("right");
    console.log("sidebar geometry", JSON.stringify({ viewport: size, main, panel, search }));
    await captureLayoutThemes(page, process.env.PWA156_PROOF_DIR, `connections-${size}`);
    if (size === 768) {
      for (const mode of ["tree", "cards"] as const) {
        if (mode === "tree") await page.getByRole("button", { name: /^web-prod-01 Unknown Connect$/ }).click();
        else {
          await page.getByRole("button", { name: "Catalogue view and actions", exact: true }).click();
          await page.getByRole("menuitem", { name: "Cards", exact: true }).click();
          await page.getByTestId(/^host-card-/).first().getByRole("button", { name: /select/i }).click();
        }
        const resizer = page.locator('[role="slider"]:not([aria-label])').first();
        const inspector = resizer.locator("xpath=preceding-sibling::div[1]");
        await expect(resizer).toBeVisible();
        const before = (await inspector.boundingBox())!;
        const catalogue = (await page.getByPlaceholder(/search targets/i).boundingBox())!;
        const cataloguePane = (await resizer.locator("xpath=following-sibling::div[1]").boundingBox())!;
        for (const box of [before, cataloguePane, catalogue]) {
          expect(box.x).toBeGreaterThanOrEqual(main.x);
          expect(box.x + box.width).toBeLessThanOrEqual(main.x + main.width + 1);
          expect(box.y + box.height).toBeLessThanOrEqual(main.y + main.height + 1);
        }
        expect(catalogue.width).toBeGreaterThanOrEqual(140);
        expect(catalogue.y).toBeGreaterThanOrEqual(before.y + before.height);
        await resizer.press("ArrowUp");
        await expect.poll(async () => (await inspector.boundingBox())!.height).toBe(before.height - 20);
        expect((await inspector.boundingBox())!.width).toBe(before.width);
        console.log("stacked inspector geometry", JSON.stringify({ mode, before, after: await inspector.boundingBox(), cataloguePane, catalogue }));
        await captureLayoutThemes(page, process.env.PWA156_PROOF_DIR, `repair-${mode}-768`);
        await inspector.getByRole("button", { name: "Close", exact: true }).click();
        expect(await viewer.evaluate((element, node) => element === node, original)).toBe(true);
        expect(opens).toHaveLength(1);
        expect(closes).toHaveLength(0);
      }
      await page.getByRole("button", { name: "Catalogue view and actions", exact: true }).click();
      await page.getByRole("menuitem", { name: "Tree", exact: true }).click();
    }
  }
  const navigate = async (section: string, path: string) => {
    const link = page.locator(`a[href="${path}"]`).filter({ visible: true });
    if (!await link.count()) await sidebar.getByRole("button", { name: section, exact: true }).click();
    if (await link.count()) await link.first().click();
    else {
      const labels: Record<string, RegExp> = {
        "/audit-logs": /audit log/i, "/observability": /^observability$/i,
        "/inventory/resources": /^resources$/i, "/administration/settings": /system settings/i,
        "/connections": /^workspace$/i,
      };
      await page.getByRole("menu").getByRole("button", { name: labels[path] }).click();
      await page.keyboard.press("Escape");
    }
    await expect(page).toHaveURL(new RegExp(`${path}$`));
    await expect(page.getByTestId("page-not-found")).toHaveCount(0);
  };
  for (const collapsed of [false, true]) {
    if (collapsed) { await toggle.click(); await expect.poll(width).toBe(56); }
    for (const [section, path] of [
      ["Administration", "/audit-logs"], ["Administration", "/observability"],
      ["Inventory", "/inventory/resources"], ["Administration", "/administration/settings"],
      ["Connections", "/connections"],
    ]) {
      await navigate(section!, path!);
      await expect.poll(width).toBe(collapsed ? 56 : 256);
      await toggle.click();
      await expect.poll(width).toBe(collapsed ? 256 : 56);
      await toggle.click();
      await expect.poll(width).toBe(collapsed ? 56 : 256);
      if (path === "/audit-logs") {
        await page.setViewportSize({ width: collapsed ? 768 : 960, height: 900 });
        await expect.poll(width).toBe(collapsed ? 56 : 256);
        await captureLayoutThemes(page, process.env.PWA156_PROOF_DIR, collapsed ? "audit-collapsed" : "audit-expanded");
      }
    }
  }
  await toggle.click();
  await expect.poll(width).toBe(256);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole("button", { name: "Back to connections", exact: true }).click();
  const mobileToggle = page.locator("#mobile-navigation-toggle");
  await mobileToggle.click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(mobileToggle).toBeFocused();
  await mobileToggle.click();
  await navigate("Administration", "/audit-logs");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await mobileToggle.click();
  await navigate("Connections", "/connections");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await captureLayoutThemes(page, process.env.PWA156_PROOF_DIR, "connections-mobile");
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect.poll(width).toBe(256);
  await page.getByRole("button", { name: /show.*open tab/i }).click();
  await expect(workspace).toBeVisible();
  expect(opens).toHaveLength(1);
  expect(closes).toHaveLength(0);
  expect(await viewer.evaluate((element, node) => element === node, original)).toBe(true);
  await expect(viewer.locator(".xterm-rows")).toContainText("Sidebar retained terminal");
  await viewer.locator(".xterm-helper-textarea").press("a");
  await expect.poll(() => inputs.length).toBe(1);
  expect(inputs).toEqual(opens);
  console.log("sidebar retained socket", JSON.stringify({ opens, closes, inputs }));
});
