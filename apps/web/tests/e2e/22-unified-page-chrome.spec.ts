import { captureLayoutThemes, expect, expectManagementCanvas, expectRoutedPage, test } from "./fixtures";

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test(`shared field labels align controls with and without help actions: ${viewport.name}`, async ({ appPage: page }) => {
    await page.setViewportSize(viewport);
    await page.goto("/inventory/templates/new");
    await expectRoutedPage(page);
    const status = page.locator("#meta-status");
    const description = page.locator("#meta-description");
    await expect(status).toBeVisible();
    await expect(description).toBeVisible();
    if (viewport.width >= 640) {
      const statusBox = await status.boundingBox();
      const descriptionBox = await description.boundingBox();
      expect(statusBox).not.toBeNull();
      expect(descriptionBox).not.toBeNull();
      expect(Math.abs(statusBox!.y - descriptionBox!.y)).toBeLessThanOrEqual(1);
    }
    for (const label of await page.locator('[data-slot="form-field"] > label').all()) {
      const layout = await label.evaluate(element => ({ height: element.clientHeight, content: element.scrollHeight }));
      expect(layout.height).toBeGreaterThanOrEqual(32);
      expect(layout.content).toBeLessThanOrEqual(layout.height);
    }
    await captureLayoutThemes(page, process.env.PIPING_UI_PROOF_DIR, `field-labels-${viewport.name}`);
  });
  test(`ordinary connections browsing shares the canvas before opening a session: ${viewport.name}`, async ({ appPage: page }) => {
    await page.setViewportSize(viewport);
    await page.goto("/connections");
    await expectRoutedPage(page);
    await expectManagementCanvas(page);
    await expect(page.getByPlaceholder(/search|поиск/i).first()).toBeVisible();
    await captureLayoutThemes(page, process.env.PIPING_UI_PROOF_DIR, `connections-browser-${viewport.name}`);
  });
  test(`management families share one rendered canvas and section-navigation design: ${viewport.name}`, async ({ appPage: page }) => {
    test.setTimeout(180_000);
    await page.setViewportSize(viewport);
    for (const route of [
      "/inventory/templates", "/inventory/templates/new", "/inventory/custom-fields",
      "/inventory/resources?entity=Sites", "/connections/config", "/connections/proxies",
      "/observability", "/playbooks", "/playbooks/new", "/access-control",
      "/user/settings", "/administration/settings", "/administration/backup",
      "/administration/license", "/administration/webhooks", "/administration/branding",
    ]) {
      await test.step(route, async () => {
        await page.goto(route);
        await expectRoutedPage(page);
        await expectManagementCanvas(page);
        await captureLayoutThemes(page, process.env.PIPING_UI_PROOF_DIR,
          `${route.slice(1).replaceAll(/[/?=&]/g, "-")}-${viewport.name}`);
      });
    }
  });
  test(`map shares the Workspace shell heading: ${viewport.name}`, async ({ appPage: page }) => {
    await page.setViewportSize(viewport);
    await page.goto("/connections");
    await expectRoutedPage(page);
    await page.getByRole("button", { name: /^(Map|Карта)$/ }).click();
    await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 })).toHaveText(/Connections Workspace|Рабочее пространство/i);
    await expect(page.getByRole("main").getByRole("heading", { level: 1 })).toHaveCount(0);
    const unplaced = page.getByRole("button", { name: /Unplaced targets \(2\)|Без координат \(2\)/ });
    await unplaced.click();
    const missingList = page.getByRole("dialog");
    await expect(missingList.getByText("app-node-02", { exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(missingList).toBeHidden();
    await expect(unplaced).toBeFocused();
    await unplaced.click();
    await missingList.getByRole("button", { name: /app-node-02/ }).click();
    await expect(page.getByRole("heading", { name: "app-node-02", exact: true })).toBeVisible();
    const geometry = await page.evaluate(() => ({ width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth }));
    expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.width + 1);
  });
  test(`inventory search and entity navigation remain usable in the shared toolbar: ${viewport.name}`, async ({ appPage: page }) => {
    await page.setViewportSize(viewport);
    await page.goto("/inventory/resources?entity=Sites");
    await expectRoutedPage(page);
    const header = page.locator('[data-slot="app-header"]');
    await expect(header.getByRole("heading", { level: 1 })).toHaveCount(1);
    await expect(page.getByRole("main").getByRole("heading", { level: 1 })).toHaveCount(0);
    const search = page.getByPlaceholder(/search sites|поиск площадки/i);
    if (viewport.width >= 768) {
      expect((await header.boundingBox())?.height).toBe(56);
    }
    await expect(page.getByRole("combobox", { name: /DCIM/i })).toBeVisible();
    await expect(search).toBeVisible();
    await search.fill("Amsterdam");
    const table = page.getByRole("table");
    await expect(table.getByText("Amsterdam Edge", { exact: true })).toBeVisible();
    await expect(table.getByText("Frankfurt DC", { exact: true })).toHaveCount(0);
    await expect(search).toHaveValue("Amsterdam");
    const geometry = await page.evaluate(() => ({ width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth }));
    expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.width + 1);
  });
}
