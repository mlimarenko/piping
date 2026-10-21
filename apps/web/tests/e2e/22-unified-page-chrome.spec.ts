import { expect, expectRoutedPage, test } from "./fixtures";

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
]) {
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
  test(`inventory controls remain usable without a permanent mobile control stack: ${viewport.name}`, async ({ appPage: page }) => {
    await page.setViewportSize(viewport);
    await page.goto("/inventory/resources?entity=Sites");
    await expectRoutedPage(page);
    const header = page.locator('[data-slot="app-header"]');
    await expect(header.getByRole("heading", { level: 1 })).toHaveCount(1);
    await expect(page.getByRole("main").getByRole("heading", { level: 1 })).toHaveCount(0);
    const disclosure = page.getByRole("button", { name: /navigation and search|навигация и поиск/i });
    const search = page.getByPlaceholder(/search sites|поиск площадки/i);
    if (viewport.width < 768) {
      await expect(disclosure).toHaveAttribute("aria-expanded", "false");
      await expect(search).toBeHidden();
      await disclosure.click();
      await expect(disclosure).toHaveAttribute("aria-expanded", "true");
    } else {
      await expect(disclosure).toHaveCount(0);
      expect((await header.boundingBox())?.height).toBe(56);
    }
    await expect(page.getByRole("combobox", { name: /DCIM/i })).toBeVisible();
    await expect(search).toBeVisible();
    await search.fill("Amsterdam");
    const table = page.getByRole("table");
    await expect(table.getByText("Amsterdam Edge", { exact: true })).toBeVisible();
    await expect(table.getByText("Frankfurt DC", { exact: true })).toHaveCount(0);
    if (viewport.width < 768) {
      await disclosure.click();
      await expect(search).toBeHidden();
      await expect(table.getByText("Amsterdam Edge", { exact: true })).toBeVisible();
      await disclosure.click();
      await expect(search).toHaveValue("Amsterdam");
    }
    const geometry = await page.evaluate(() => ({ width: document.documentElement.clientWidth, scrollWidth: document.documentElement.scrollWidth }));
    expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.width + 1);
  });
}
