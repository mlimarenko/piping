import { captureLayoutThemes, expect, expectRoutedPage, test } from "./fixtures";
import { mkdir, writeFile } from "node:fs/promises";

const proof = process.env.PWA155_PROOF_DIR;
const base = "/api/v1/monitoring/exporter-assignments";
interface RepairWindow extends Window {
  __pwa155Repair: {
    failReads: boolean;
    releaseRead?: () => void;
    releaseWrite?: () => void;
    writes: Array<{ groups: Record<string, unknown> }>;
    events: string[];
  };
}
async function repairReceipt(name: string, data: unknown) {
  if (!proof) return;
  await mkdir(proof, { recursive: true });
  await writeFile(`${proof}/${name}.json`, JSON.stringify(data, null, 2));
}
const record = {
  id: "synthetic-assignment-155", name: "Synthetic edge metrics", exporter_id: "exp-node-0000-0000-0000-000000000001",
  exporter_name: "Node Exporter", exporter_slug: "node", scope_type: "connection_target", scope_id: "synthetic-target-155",
  enabled: true, weight: 100, job: null, target: null, port: null, scheme: null, metrics_path: null, labels: { fixture: "PWA-155" },
  created_at: "2026-10-16T00:00:00Z", updated_at: "2026-10-16T00:00:00Z",
};

// Synthetic transport at the browser boundary, over the real demo SPA and MSW.
// No customer services, credentials or production records are involved.
test.beforeEach(async ({ page }, info) => {
  await page.addInitScript(({ base, record, scenario }) => {
    if (scenario.includes("Russian")) window.localStorage.setItem("i18nextLng", "ru");
    const original = window.fetch.bind(window);
    let saved = record;
    let removed = false;
    let settingsFailed = false;
    let previewFailed = false;
    const controls: RepairWindow["__pwa155Repair"] = { failReads: false, writes: [], events: [] };
    (window as unknown as RepairWindow).__pwa155Repair = controls;
    let settingsReads = 0;
    let settingsGroups: Record<string, unknown> = { contacts: true, description: false, hardware: false, identifiers: false, location: false, network: false, role: false, status: false, tenant: false, device_statuses: ["active", "planned"], site_statuses: ["active"] };
    const longRows = Array.from({ length: 62 }, (_, index) => ({ ...record, id: `${record.id}-${index}`, name: `A very long assignment name — Synthetic regional infrastructure Linux telemetry collection policy ${String(index).padStart(2, "0")}`, exporter_name: "Synthetic exceptionally long exporter profile for infrastructure telemetry", scope_id: `synthetic-target-${index}`, enabled: index % 2 === 0 }));
    window.fetch = async (input, init) => {
      const request = new Request(input, init);
      const url = new URL(request.url);
      if (scenario.includes("repair editor") && request.method === "GET" && (url.pathname === base || url.pathname === "/api/v1/monitoring/exporters")) {
        controls.events.push(`GET ${url.pathname} ${controls.failReads ? 503 : 200}`);
        if (controls.failReads) return Response.json({ message: "synthetic background read failed" }, { status: 503 });
      }
      if (scenario.includes("repair settings") && url.pathname === "/api/v1/monitoring/sd/label-settings") {
        if (request.method === "GET") {
          settingsReads++;
          const snapshot = structuredClone(settingsGroups);
          if (settingsReads === 2) return new Promise<Response>(resolve => { controls.events.push("old GET deferred"); controls.releaseRead = () => { controls.events.push("old GET completed"); resolve(Response.json({ groups: snapshot })); }; });
          return Response.json({ groups: snapshot });
        }
        if (request.method === "PUT") {
          const body = await request.json() as { groups: Record<string, unknown> };
          controls.writes.push(body);
          controls.events.push(`PUT ${controls.writes.length} started`);
          if (controls.writes.length === 1) await new Promise<void>(resolve => { controls.releaseWrite = resolve; });
          settingsGroups = body.groups;
          controls.events.push(`PUT ${controls.writes.length} acknowledged`);
          return Response.json({ groups: settingsGroups });
        }
      }
      if (scenario.includes("repair populated") && url.pathname === base && request.method === "GET") return Response.json({ items: longRows, total: 62, limit: 200, offset: 0 });
      if (scenario.includes("repair populated") && url.pathname === "/api/v1/connections/targets") return Response.json({ items: longRows.map((row, index) => ({ id: row.scope_id, name: `Synthetic gateway ${String(index).padStart(2, "0")} · regional telemetry destination`, target_kind: "device", address: `192.0.2.${index + 1}`, status: "active" })), total: 62, limit: 50000, offset: 0 });
      if (url.pathname === "/api/v1/auth/me" && scenario.includes("READ-only")) {
        const response = await original(request);
        const user = await response.json();
        return Response.json({ ...user, roles: [], capabilities: user.capabilities.filter((capability: { action: string }) => capability.action === "READ") });
      }
      if (url.pathname === base && request.method === "GET" && scenario.includes("state loading")) return new Promise<Response>(() => {});
      if (url.pathname === base && request.method === "GET" && scenario.includes("state error")) return Response.json({ message: "synthetic unavailable" }, { status: 500 });
      if (url.pathname === base && request.method === "GET" && scenario.includes("state empty")) return Response.json({ items: [], total: 0, limit: 200, offset: 0 });
      if (url.pathname === "/api/v1/connections/targets" && scenario.includes("lookup-denied")) return Response.json({ message: "synthetic lookup denied" }, { status: 403 });
      if (url.pathname === "/api/v1/monitoring/sd/label-settings" && request.method === "PUT" && scenario.includes("save recovery") && !settingsFailed) { settingsFailed = true; return Response.json({ message: "synthetic save rejected" }, { status: 500 }); }
      if (url.pathname === "/api/v1/monitoring/sd/preview" && scenario.includes("preview retry") && !previewFailed) { previewFailed = true; return Response.json({ message: "synthetic preview unavailable" }, { status: 500 }); }
      if (url.pathname === base && request.method === "GET") return Response.json({ items: removed ? [] : [saved], total: removed ? 0 : 1, limit: 200, offset: 0 });
      if (url.pathname === `${base}/${record.id}` && request.method === "PUT") { saved = { ...saved, ...await request.json() }; return Response.json(saved); }
      if (url.pathname === `${base}/${record.id}` && request.method === "DELETE") { removed = true; return new Response(null, { status: 204 }); }
      if (url.pathname === "/api/v1/connections/targets" && request.method === "GET") return Response.json({ items: [{ id: "synthetic-target-155", name: "Synthetic edge gateway", target_kind: "device", address: "192.0.2.155", status: "active" }], total: 1, limit: 50000, offset: 0 });
      return original(request);
    };
  }, { base, record, scenario: info.title });
  await page.goto(info.title.includes("Russian") ? "/observability?lang=ru" : "/observability");
  await expectRoutedPage(page);
});

for (const viewport of [
  { name: "desktop1440", width: 1440, height: 900 },
  { name: "desktop1920", width: 1920, height: 1080 },
  { name: "mobile390", width: 390, height: 844 },
]) {
  test(`collection, routed detail/editor, collector and settings at ${viewport.name}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    const main = page.locator("#main-content");
    const assignment = main.getByRole("link", { name: record.name });
    await expect(assignment).toBeVisible();
    await expect(main.getByText(/Synthetic edge gateway/)).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
    await captureLayoutThemes(page, proof, `assignments-${viewport.name}`);
    await assignment.click();
    await expect(page).toHaveURL(new RegExp(`/exporter-assignments/${record.id}`));
    await expect(main.getByText(record.id, { exact: true })).toBeVisible();
    await captureLayoutThemes(page, proof, `detail-${viewport.name}`);
    await page.getByRole("button", { name: /^edit|^редактировать/i }).click();
    const name = main.getByRole("textbox", { name: /assignment name|название назначения/i });
    await expect(name).toHaveValue(record.name);
    await name.fill("Synthetic draft retained");
    await captureLayoutThemes(page, proof, `editor-${viewport.name}`);
    await main.getByRole("button", { name: /^cancel|^отмена/i }).click();
    const dirty = page.getByRole("alertdialog");
    await expect(dirty).toBeVisible();
    await captureLayoutThemes(page, proof, `dirty-cancel-${viewport.name}`);
    await dirty.getByRole("button", { name: /continue editing|продолжить редактирование/i }).click();
    await expect(name).toHaveValue("Synthetic draft retained");
    await main.getByRole("button", { name: /^save|^сохранить/i }).click();
    await expect(page.getByRole("heading", { name: "Synthetic draft retained" })).toBeVisible();
    await main.getByRole("link", { name: /^back|^назад/i }).click();
    await main.getByRole("link", { name: /connect collector|подключение коллектора/i }).click();
    const load = main.getByRole("button", { name: /load sample|загрузить пример/i });
    await expect(load).toBeVisible();
    await captureLayoutThemes(page, proof, `collector-deferred-${viewport.name}`);
    await load.click();
    await expect(main.getByRole("button", { name: /copy sample output|скопировать пример/i })).toBeVisible();
    await captureLayoutThemes(page, proof, `collector-populated-${viewport.name}`);
    await main.getByRole("link", { name: /discovery settings|настройки выдачи/i }).click();
    await expect(main.getByRole("textbox", { name: /device statuses|статусы устройств/i })).toBeVisible();
    await captureLayoutThemes(page, proof, `settings-${viewport.name}`);
    await main.getByRole("link", { name: /inspector metrics|метрики в инспекторе/i }).click();
    await expect(main.getByRole("tab", { name: /profiles|профили/i })).toBeVisible();
    await captureLayoutThemes(page, proof, `inspector-${viewport.name}`);
    const dimensions = await page.evaluate(() => ({ width: document.documentElement.clientWidth, scroll: document.documentElement.scrollWidth }));
    expect(dimensions.scroll).toBeLessThanOrEqual(dimensions.width + 1);
  });
}

for (const state of ["empty", "loading", "error", "lookup-denied", "READ-only"]) {
  test(`state ${state} has truthful accessible collection feedback`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const main = page.locator("#main-content");
    if (state === "empty") await expect(main.getByText("No exporter assignments")).toBeVisible();
    else if (state === "loading") await expect(main.getByRole("status")).toBeVisible();
    else if (state === "error") await expect(main.getByRole("button", { name: "Retry" })).toBeVisible();
    else {
      await expect(main.getByRole("link", { name: record.name })).toBeVisible();
      if (state === "lookup-denied") await expect(main.getByText(/Name unavailable.*synthetic-target-155/)).toBeVisible();
      if (state === "READ-only") await expect(main.getByRole("link", { name: "Add assignment" })).toHaveCount(0);
    }
    await captureLayoutThemes(page, proof, `collection-${state}-mobile390`);
  });
}

test("collector preview retry and settings save recovery keep their visible drafts", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const main = page.locator("#main-content");
  await main.getByRole("link", { name: "Connect collector" }).click();
  await main.getByRole("button", { name: "Load sample" }).click();
  await expect(main.getByText("Failed to load data")).toBeVisible();
  await captureLayoutThemes(page, proof, "collector-error-desktop1440");
  await main.getByRole("button", { name: "Retry", exact: true }).last().click();
  await expect(main.getByRole("button", { name: "Copy sample output" })).toBeVisible();
  await main.getByRole("link", { name: "Discovery settings" }).click();
  const location = main.getByRole("checkbox", { name: "Location", exact: true });
  await location.click();
  const saveError = main.getByText(/Could not save/);
  await expect(saveError).toBeVisible();
  await saveError.scrollIntoViewIfNeeded();
  await expect(saveError).toBeInViewport();
  const draftValue = await location.isChecked();
  await captureLayoutThemes(page, proof, "settings-failed-save-desktop1440");
  await main.getByRole("button", { name: "Retry", exact: true }).click();
  const savedMessage = main.getByText("Label settings saved");
  await expect(savedMessage).toBeVisible();
  await savedMessage.scrollIntoViewIfNeeded();
  expect(await location.isChecked()).toBe(draftValue);
  await captureLayoutThemes(page, proof, "settings-recovered-desktop1440");
});

test("state loading routed record pages retain readable exits", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  for (const suffix of ["", "/edit"]) {
    await page.goto(`/observability/exporter-assignments/${record.id}${suffix}`);
    await expect(page.locator("#main-content").getByRole("status")).toBeVisible();
    await expect(page.getByRole("link", { name: "Back" }).or(page.getByRole("button", { name: "Back" }))).toBeVisible();
    await captureLayoutThemes(page, proof, `record-${suffix ? "editor" : "detail"}-loading-mobile390`);
  }
});

test("repair populated mobile keeps long assignment rows compact and destinations immediately readable", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const table = page.locator('[data-table-id="observability.exporters"]');
  await expect(table.getByText(/Synthetic gateway 00/)).toBeVisible();
  const firstRow = table.locator("tbody tr").first();
  const destination = firstRow.getByText(/Synthetic gateway 00/);
  const actions = firstRow.getByRole("button", { name: /Actions:/i });
  const rowBox = await firstRow.boundingBox();
  // The rejected 390px candidate used ~12 lines for the assignment label alone.
  // This fixture allows two destination lines plus compact name/profile summaries.
  expect(rowBox?.height).toBeLessThanOrEqual(128);
  await expect(destination).toBeInViewport();
  await expect(actions).toBeInViewport();
  const viewport = table.locator('[data-table-scroll-viewport="true"]');
  const visibleRows = await viewport.evaluate(element => {
    const bounds = element.getBoundingClientRect();
    return [...element.querySelectorAll("tbody tr")].filter(row => { const box = row.getBoundingClientRect(); return box.top >= bounds.top && box.bottom <= bounds.bottom; }).length;
  });
  expect(visibleRows).toBeGreaterThanOrEqual(2);
  await repairReceipt("mobile-populated62", { viewport: { width: 390, height: 844 }, fixtureRows: 62, rowHeight: rowBox?.height, fullyVisibleRows: visibleRows, destination: await destination.textContent(), actionReachable: await actions.isVisible() });
  await captureLayoutThemes(page, proof, "repair-populated62-mobile390");
  await actions.click();
  await expect(page.getByRole("menuitem", { name: "Details" })).toBeVisible();
  await page.keyboard.press("Escape");
  for (const width of [1440, 1920]) {
    await page.setViewportSize({ width, height: 900 });
    await expect(table.getByRole("columnheader", { name: /Exporter profile/ })).toBeVisible();
    await captureLayoutThemes(page, proof, `repair-populated62-desktop${width}`);
  }
});

test("repair editor preserves exact dirty draft through reconnect error and retry", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.getByRole("link", { name: record.name }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const name = page.getByRole("textbox", { name: /Assignment name/ });
  await name.fill("  Synthetic retained after reconnect  ");
  await page.clock.install();
  await page.clock.fastForward(61_000);
  await page.evaluate(() => { (window as unknown as RepairWindow).__pwa155Repair.failReads = true; window.dispatchEvent(new Event("offline")); });
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await page.clock.fastForward(10_000);
  const retry = page.getByRole("button", { name: "Retry", exact: true });
  await expect(retry).toBeVisible();
  await expect(name).toHaveValue("  Synthetic retained after reconnect  ");
  await retry.scrollIntoViewIfNeeded();
  await captureLayoutThemes(page, proof, "repair-editor-background-error");
  await page.evaluate(() => { (window as unknown as RepairWindow).__pwa155Repair.failReads = false; });
  await retry.click();
  await expect(retry).toHaveCount(0);
  await expect(name).toHaveValue("  Synthetic retained after reconnect  ");
  const retainedDraft = await name.inputValue();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("alertdialog")).toBeVisible();
  expect(errors).toEqual([]);
  await repairReceipt("editor-background-error", { transport: "synthetic fetch boundary in real demo SPA", events: await page.evaluate(() => (window as unknown as RepairWindow).__pwa155Repair.events), retainedDraft, dirtyDialog: await page.getByRole("alertdialog").isVisible(), pageErrors: errors });
});

test("repair populated Russian mobile keeps two destinations and their actions visible", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const table = page.locator('[data-table-id="observability.exporters"]');
  const first = table.locator("tbody tr").nth(0);
  const second = table.locator("tbody tr").nth(1);
  await expect(first.getByText(/Synthetic gateway 00/)).toBeInViewport();
  await expect(second.getByText(/Synthetic gateway 01/)).toBeInViewport();
  await expect(second.getByRole("button", { name: /Действия:/i })).toBeInViewport();
  expect((await first.boundingBox())?.height).toBeLessThanOrEqual(128);
  await captureLayoutThemes(page, proof, "repair-populated62-ru-mobile390");
  await repairReceipt("mobile-populated62-ru", { rowHeight: (await first.boundingBox())?.height, firstDestination: await first.getByText(/Synthetic gateway 00/).textContent(), secondDestinationVisible: true, secondActionsVisible: true });
});

test("repair settings ignores delayed old GET after acknowledged PUT and preserves the next full payload", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.getByRole("link", { name: "Discovery settings", exact: true }).click();
  const location = page.getByRole("checkbox", { name: "Location", exact: true });
  await expect(location).toBeVisible();
  await page.clock.install();
  await page.clock.fastForward(31_000);
  await page.evaluate(() => window.dispatchEvent(new Event("offline")));
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as RepairWindow).__pwa155Repair.releaseRead))).toBe(true);
  await location.click();
  await expect(page.getByText("Saving…", { exact: true })).toBeVisible();
  await page.getByText("Saving…", { exact: true }).scrollIntoViewIfNeeded();
  await captureLayoutThemes(page, proof, "repair-settings-pending");
  await expect.poll(() => page.evaluate(() => Boolean((window as unknown as RepairWindow).__pwa155Repair.releaseWrite))).toBe(true);
  await page.evaluate(() => (window as unknown as RepairWindow).__pwa155Repair.releaseWrite?.());
  await expect(page.getByText("Label settings saved", { exact: true })).toBeVisible();
  await page.evaluate(() => (window as unknown as RepairWindow).__pwa155Repair.releaseRead?.());
  await expect(location).toBeChecked();
  await page.getByRole("checkbox", { name: "Tenant", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as RepairWindow).__pwa155Repair.writes)).toHaveLength(2);
  const writes = await page.evaluate(() => (window as unknown as RepairWindow).__pwa155Repair.writes);
  expect(writes[1]?.groups).toMatchObject({ location: true, tenant: true, contacts: true, device_statuses: ["active", "planned"], site_statuses: ["active"] });
  await expect(page.getByText("Label settings saved", { exact: true })).toBeVisible();
  await page.getByText("Label settings saved", { exact: true }).scrollIntoViewIfNeeded();
  await captureLayoutThemes(page, proof, "repair-settings-acknowledged-old-get");
  expect(errors).toEqual([]);
  await repairReceipt("settings-read-write-order", { transport: "synthetic fetch boundary in real demo SPA", events: await page.evaluate(() => (window as unknown as RepairWindow).__pwa155Repair.events), writes, locationChecked: await location.isChecked(), pageErrors: errors });
});
