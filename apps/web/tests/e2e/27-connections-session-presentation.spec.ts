import { apiRequest, captureLayoutThemes, expect, test, waitForConnectionsPageReady } from "./fixtures";

test("standalone split guard explains how to retain the existing work group", async ({ page }) => {
  const ids = ["guard-1", "guard-2", "guard-3"];
  await page.routeWebSocket(/\/guard-proof\//, socket => socket.send("\r\nGuard fixture\r\n"));
  await page.addInitScript(ids => {
    (window as Window & { __PIPING_DETACHED_GROUP_HANDOFF__?: unknown }).__PIPING_DETACHED_GROUP_HANDOFF__ = ids.map((id, index) => ({
      sessionId: id, hostId: id, hostName: `Guard ${index + 1}`, protocol: "ssh",
      websocketUrl: `ws://${location.host}/guard-proof/${id}`, token: "fixture",
    }));
  }, ids);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/connections/detached-group?ids=${ids.join(",")}`);
  const workspace = page.getByTestId("session-workspace");
  const actions = workspace.getByRole("button", { name: "Pane 1 actions", exact: true });
  await actions.focus();
  await actions.press("Enter");
  await expect(page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true })).not.toHaveAttribute("aria-disabled", "true");
  await page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true }).click();
  await workspace.getByRole("combobox", { name: "Session in pane 2" }).selectOption(ids[1]!);
  const resize = workspace.getByRole("separator", { name: "Resize session panes" });
  await resize.press("ArrowRight");
  const ratio = await resize.getAttribute("aria-valuenow");
  await workspace.getByRole("tab", { name: "SSH Guard 3", exact: true }).click();
  await actions.focus();
  await expect(actions).toHaveAccessibleDescription(/To change the layout, return to Work group/);
  await expect(page.getByRole("tooltip")).toContainText("Splitting this session would replace the saved group.");
  await actions.press("Enter");
  const blocked = page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true });
  await expect(blocked).toHaveAttribute("aria-disabled", "true");
  await expect(page.getByRole("menuitem", { name: "Split pane 1 stacked", exact: true })).toHaveAttribute("aria-disabled", "true");
  await expect(page.getByRole("menu")).toContainText("To change the layout, return to Work group.");
  await captureLayoutThemes(page, process.env.PWA119_PROOF_DIR, "split-guard-desktop");
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 390, height: 844 });
  await actions.press("Enter");
  await expect(page.getByRole("menuitem", { name: "Split pane 1 stacked", exact: true })).toHaveAttribute("aria-disabled", "true");
  await captureLayoutThemes(page, process.env.PWA119_PROOF_DIR, "split-guard-phone");
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 1440, height: 900 });
  await workspace.getByRole("tab", { name: "Work group · 2", exact: true }).click();
  await expect(workspace.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue(ids[0]!);
  await expect(workspace.getByRole("combobox", { name: "Session in pane 2" })).toHaveValue(ids[1]!);
  await expect(resize).toHaveAttribute("aria-valuenow", ratio!);
  await actions.press("Enter");
  await expect(page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true })).not.toHaveAttribute("aria-disabled", "true");
});

test("group-member rail focus survives standalone navigation and preference rehydration", async ({ page }) => {
  const preferences = new Map<string, unknown>();
  await page.exposeFunction("persistFixtureSetting", (method: string, key: string, value?: unknown) => {
    if (method === "PUT") preferences.set(key, value);
    return { found: preferences.has(key), value: preferences.get(key) };
  });
  await page.addInitScript(() => {
    const fetch = window.fetch.bind(window);
    const store = (window as Window & { persistFixtureSetting?: (method: string, key: string, value?: unknown) => Promise<{ found: boolean; value: unknown }> }).persistFixtureSetting;
    if (!store) throw new Error("Preference backend fixture is not installed");
    // Preserve successful settings writes in a backend fixture across full page
    // reloads. Other APIs still use the demo backend; its reset is not a restore.
    window.fetch = async (input, init) => {
      const request = new Request(input instanceof Request ? input.clone() : input, init);
      const url = new URL(request.url);
      const response = await fetch(input, init);
      const write = url.pathname.match(/^\/api\/v1\/settings\/user\/(.+)$/);
      if (write && request.method === "PUT" && response.ok) {
        const body = await request.json() as { value: unknown };
        await store("PUT", decodeURIComponent(write[1]!), body.value);
      }
      if (request.method !== "GET" || !url.pathname.startsWith("/api/v1/settings/effective") || !response.ok) return response;
      const body = await response.json() as { key?: string; value?: unknown; settings?: { key: string; value: unknown }[] };
      const rows = body.settings ?? (body.key ? [body as { key: string; value: unknown }] : []);
      for (const row of rows) {
        const saved = await store("GET", row.key);
        if (saved.found) row.value = saved.value;
      }
      return new Response(JSON.stringify(body), { status: response.status, headers: response.headers });
    };
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/connections");
  await waitForConnectionsPageReady(page);
  const workspace = page.getByTestId("session-workspace");
  for (let index = 0; index < 3; index++) {
    await page.getByTestId(/^quick-access-/).nth(index).click();
    await page.getByRole("menuitem").filter({ hasText: /SSH/ }).first().click();
    await expect(workspace.locator("[data-session-pane-viewer]")).toHaveCount(index + 1);
  }
  const ids = await workspace.locator("[data-session-pane-viewer]").evaluateAll(nodes => nodes.map(node => node.getAttribute("data-session-pane-viewer")!));
  await workspace.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true }).click();
  await workspace.getByRole("combobox", { name: "Session in pane 1" }).selectOption(ids[0]!);
  await workspace.getByRole("combobox", { name: "Session in pane 2" }).selectOption(ids[1]!);
  await workspace.getByRole("button", { name: "Pane 2 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Maximize pane", exact: true }).click();
  const savedPanel = () => preferences.get("connections.workspace.panel") as { paneLayout: { focusedPaneId: string; maximizedPaneId: string; root: unknown }; activeSession: { sessionId: string } } | undefined;
  await expect.poll(() => savedPanel()?.paneLayout.maximizedPaneId).toBe("pane-2");
  const root = savedPanel()!.paneLayout.root;
  await workspace.getByRole("button", { name: "Collapse", exact: true }).click();
  await workspace.getByRole("tab").nth(0).click();
  await workspace.getByRole("tab", { name: /^SSH / }).click();
  await expect.poll(() => savedPanel()).toMatchObject({ paneLayout: { focusedPaneId: "pane-1", maximizedPaneId: "pane-1", root }, activeSession: { sessionId: ids[2] } });
  const saved = structuredClone(savedPanel());
  await page.reload();
  await waitForConnectionsPageReady(page);
  await expect(workspace.getByRole("tab", { name: "Work group · 2" })).toBeVisible();
  await workspace.getByRole("tab", { name: "Work group · 2" }).click();
  await expect(workspace.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue(ids[0]!);
  await expect(workspace.getByRole("button", { name: "Restore panes", exact: true })).toBeVisible();
  await expect(workspace.getByRole("combobox", { name: "Session in pane 2" })).toHaveCount(0);
  expect(savedPanel()!.paneLayout).toEqual(saved!.paneLayout);
});

test("right dock collapse retains its edge and restores the same viewer", async ({ appPage: page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.routeWebSocket(/\/ws\//, socket => {
    socket.send("\r\nLocal terminal ready\r\n");
    socket.onMessage(message => socket.send(message));
  });
  await page.getByTestId(/^quick-access-/).first().click();
  await page.getByRole("menuitem").filter({ hasText: /SSH/ }).first().click();
  const workspace = page.getByTestId("session-workspace");
  await expect(workspace).toBeVisible();
  const right = workspace.getByRole("button", { name: /dock.*right|справа/i });
  if (await right.count()) await right.first().click();
  const viewer = workspace.locator("[data-session-pane-viewer]").first();
  const original = await viewer.elementHandle();
  const before = (await workspace.boundingBox())!;
  await captureLayoutThemes(page, process.env.PWA119_PROOF_DIR, "right-expanded");
  await workspace.getByRole("button", { name: /^collapse$|^свернуть$/i }).click();
  await expect.poll(async () => Math.round((await workspace.boundingBox())!.width)).toBe(36);
  const collapsed = (await workspace.boundingBox())!;
  expect(Math.abs(collapsed.x + collapsed.width - before.x - before.width)).toBeLessThanOrEqual(1);
  await expect.poll(async () => {
    const state = await apiRequest(page, "GET", "/api/v1/settings/effective/connections.workspace.panel");
    return (await state.json() as { value: { dockPosition: string; expanded: boolean } }).value;
  }).toMatchObject({ dockPosition: "right", expanded: false });
  await captureLayoutThemes(page, process.env.PWA119_PROOF_DIR, "right-collapsed");
  await workspace.getByRole("button", { name: /^expand$|^развернуть$/i }).click();
  await expect.poll(async () => Math.round((await workspace.boundingBox())!.width)).toBe(Math.round(before.width));
  expect(await viewer.evaluate((element, node) => element === node, original)).toBe(true);
});

test("workspace persists group assignments independently of standalone and rail navigation", async ({ page }) => {
  const opens: string[] = [], closes: string[] = [];
  await page.routeWebSocket(/\/presentation-proof\//, socket => {
    opens.push(socket.url());
    socket.onClose(() => closes.push(socket.url()));
    socket.send("\r\nWorkspace buffer stays alive\r\n");
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/connections");
  await waitForConnectionsPageReady(page);
  // Route the demo response's relay to the fixture transport. A non-demo URL
  // passes through the demo wrapper to the native browser WebSocket.
  await page.evaluate(() => {
    const Socket = window.WebSocket;
    window.WebSocket = class extends Socket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(String(url).replace("wss://demo.invalid/relay/ssh/", "ws://127.0.0.1:4289/presentation-proof/"), protocols);
      }
    };
  });
  const workspace = page.getByTestId("session-workspace");
  for (let index = 0; index < 3; index++) {
    await page.getByTestId(/^quick-access-/).nth(index).click();
    await page.getByRole("menuitem").filter({ hasText: /SSH/ }).first().click();
    await expect.poll(() => opens.length).toBe(index + 1);
  }
  const ids = await workspace.locator("[data-session-pane-viewer]").evaluateAll(nodes => nodes.map(node => node.getAttribute("data-session-pane-viewer")!));
  await workspace.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true }).click();
  await workspace.getByRole("combobox", { name: "Session in pane 1" }).selectOption(ids[0]!);
  await workspace.getByRole("combobox", { name: "Session in pane 2" }).selectOption(ids[1]!);
  const savedLayout = async () => {
    const response = await apiRequest(page, "GET", "/api/v1/settings/effective/connections.workspace.panel");
    return (await response.json() as { value: { paneLayout: unknown } }).value.paneLayout;
  };
  await expect.poll(savedLayout).toMatchObject({ root: { first: { sessionId: ids[0] }, second: { sessionId: ids[1] } } });
  const layout = await savedLayout();
  await workspace.getByRole("tab", { name: /^SSH / }).click();
  await expect(workspace.locator("[data-session-pane-viewer]:visible")).toHaveCount(1);
  await workspace.getByRole("tab", { name: "Work group · 2" }).click();
  await expect(workspace.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue(ids[0]!);
  await expect(workspace.getByRole("combobox", { name: "Session in pane 2" })).toHaveValue(ids[1]!);
  await expect.poll(savedLayout).toEqual(layout);
  await workspace.getByRole("button", { name: "Collapse", exact: true }).click();
  await workspace.getByRole("tab").nth(2).click();
  await expect(workspace.locator("[data-session-pane-viewer]:visible")).toHaveCount(1);
  await expect.poll(savedLayout).toEqual(layout);
  await workspace.getByRole("tab", { name: "Work group · 2" }).click();
  await workspace.getByRole("combobox", { name: "Session in pane 2" }).selectOption(ids[2]!);
  await expect.poll(savedLayout).toMatchObject({ root: { first: { sessionId: ids[0] }, second: { sessionId: ids[2] } } });
  expect(opens).toHaveLength(3);
  expect(closes).toHaveLength(0);
});

test("standalone navigation preserves the real group canvas and socket identities", async ({ page }) => {
  const ids = ["presentation-1", "presentation-2", "presentation-3"];
  const opens: string[] = [], closes: string[] = [], inputs: string[] = [];
  await page.routeWebSocket(/\/presentation-proof\//, socket => {
    opens.push(socket.url());
    socket.onClose(() => closes.push(socket.url()));
    socket.onMessage(message => { if (message === "a") inputs.push(socket.url()); });
    socket.send("\r\nRetained group buffer\r\n");
  });
  await page.addInitScript(ids => {
    (window as Window & { __PIPING_DETACHED_GROUP_HANDOFF__?: unknown }).__PIPING_DETACHED_GROUP_HANDOFF__ = ids.map((id, index) => ({
      sessionId: id, hostId: id, hostName: `Terminal ${index + 1}`, protocol: "ssh",
      websocketUrl: `ws://127.0.0.1:4289/presentation-proof/${id}`, token: "fixture",
    }));
  }, ids);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`/connections/detached-group?ids=${ids.join(",")}`);
  const workspace = page.getByTestId("session-workspace");
  await expect.poll(() => opens.length).toBe(3);
  await workspace.getByRole("button", { name: "Pane 1 actions", exact: true }).click();
  await page.getByRole("menuitem", { name: "Split pane 1 side-by-side", exact: true }).click();
  await workspace.getByRole("combobox", { name: "Session in pane 2" }).selectOption(ids[1]!);
  const separator = workspace.getByRole("separator", { name: "Resize session panes" });
  await separator.press("ArrowRight");
  const ratio = await separator.getAttribute("aria-valuenow");
  const viewers = ids.map(id => workspace.locator(`[data-session-pane-viewer="${id}"]`));
  await expect(viewers[1]!.locator(".xterm-rows")).toContainText("Retained group buffer");
  const originals = await Promise.all(viewers.map(viewer => viewer.elementHandle()));
  const selectThird = workspace.getByRole("tab", { name: "SSH Terminal 3", exact: true });
  await selectThird.click();
  await expect(workspace.locator("[data-session-pane-viewer]:visible")).toHaveCount(1);
  const grid = (await workspace.locator("[data-session-pane-grid]").boundingBox())!;
  const standalone = (await viewers[2]!.boundingBox())!;
  expect(Math.abs(standalone.width - grid.width)).toBeLessThanOrEqual(1);
  expect(Math.abs(standalone.height - grid.height)).toBeLessThanOrEqual(1);
  await viewers[2]!.locator(".xterm-helper-textarea").press("a");
  await expect.poll(() => inputs.length).toBe(1);
  expect(inputs[0]).toContain(ids[2]);
  await captureLayoutThemes(page, process.env.PWA119_PROOF_DIR, "standalone-desktop");
  await workspace.getByRole("tab", { name: "Work group · 2" }).click();
  await expect(workspace.getByRole("combobox", { name: "Session in pane 1" })).toHaveValue(ids[0]!);
  await expect(workspace.getByRole("combobox", { name: "Session in pane 2" })).toHaveValue(ids[1]!);
  await expect(separator).toHaveAttribute("aria-valuenow", ratio!);
  await expect(workspace.locator("[data-session-pane-viewer]:visible")).toHaveCount(2);
  await expect(viewers[1]!.locator(".xterm-rows")).toContainText("Retained group buffer");
  await viewers[1]!.locator(".xterm-helper-textarea").press("a");
  await expect.poll(() => inputs.length).toBe(2);
  expect(inputs[1]).toContain(ids[1]);
  await captureLayoutThemes(page, process.env.PWA119_PROOF_DIR, "group-desktop");
  await workspace.getByRole("button", { name: "Maximize pane" }).nth(1).click();
  await selectThird.click();
  await workspace.getByRole("tab", { name: "Work group · 2" }).click();
  await expect(workspace.getByRole("button", { name: "Restore panes" })).toBeVisible();
  await expect(viewers[1]!).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await selectThird.click();
  await captureLayoutThemes(page, process.env.PWA119_PROOF_DIR, "standalone-phone");
  for (const [index, viewer] of viewers.entries()) {
    expect(await viewer.evaluate((element, original) => element === original, originals[index]!)).toBe(true);
  }
  expect(opens).toHaveLength(3);
  expect(closes).toHaveLength(0);
});
