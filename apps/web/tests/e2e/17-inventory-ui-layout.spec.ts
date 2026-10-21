import { expect, expectRoutedPage, openActionsMenu, test } from "./fixtures";
import {
  inventoryCategories,
  isPseudoInventoryRailKey,
} from "../../src/features/inventory/components/inventory/inventoryCategories";

const viewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
] as const;

for (const viewport of viewports) {
  test.describe(`inventory layout at ${viewport.name}`, () => {
    test.setTimeout(180_000);
    test("row editing is discoverable without hovering", async ({ appPage: page }) => {
      await page.setViewportSize(viewport);
      await page.goto("/inventory/resources?entity=Sites");
      await expectRoutedPage(page);
      const actions = page.getByRole("table").getByRole("button", { name: /^(actions|действия):/i }).first();
      await actions.scrollIntoViewIfNeeded();
      await page.mouse.move(1, 1);
      const opacity = await actions.evaluate(element => {
        let effective = 1;
        for (let node: Element | null = element; node; node = node.parentElement) {
          effective *= Number.parseFloat(getComputedStyle(node).opacity);
        }
        return effective;
      });
      expect(opacity).toBe(1);
      const menu = await openActionsMenu(page, actions);
      await menu.getByRole("menuitem", { name: /^(edit|редактировать)$/i }).click();
      await expect(page).toHaveURL(/\/inventory\/resources\/Sites\/[^/]+\/edit/);
      await expect(page.getByTestId("entity-editor-page")).toBeVisible();
    });
    for (const category of inventoryCategories) {
      test(`${category.key} deep links expose one readable page`, async ({ appPage: page }) => {
        await page.setViewportSize(viewport);
        for (const entity of category.entities) {
          await test.step(entity, async () => {
            await page.goto(`/inventory/resources?entity=${encodeURIComponent(entity)}`);
            await expectRoutedPage(page);
            const main = page.locator("#main-content");
            await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 })).toHaveCount(1);
            await expect(main.getByRole("heading", { level: 1 })).toHaveCount(0);
            if (!isPseudoInventoryRailKey(entity)) {
              await expect(main.getByRole("table").first()).toBeVisible();
            }
            const geometry = await page.evaluate(() => ({
              width: document.documentElement.clientWidth,
              scrollWidth: document.documentElement.scrollWidth,
            }));
            expect(geometry.scrollWidth, `${entity} must not overflow the page`).toBeLessThanOrEqual(geometry.width + 1);
          });
        }
      });
    }
  });
}
