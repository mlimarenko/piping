import { captureLayoutThemes, expect, expectRoutedPage, test } from "./fixtures";

const routes = [
  "/connections/config",
  "/connections/config/new",
  "/connections/proxies",
  "/connections/proxies/new",
  "/connections/proxies/assignments/new",
  "/connections/active",
] as const;

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test.describe(`connection catalogue at ${viewport.name}`, () => {
    for (const route of routes) {
      test(`${route} has a readable, bounded page`, async ({ appPage: page }) => {
        await page.setViewportSize(viewport);
        await page.goto(route);
        await expectRoutedPage(page);
        const main = page.locator("#main-content");
        await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 })).toHaveCount(1);
        await expect(main.getByRole("heading", { level: 1 })).toHaveCount(0);
        const geometry = await page.evaluate(() => ({
          width: document.documentElement.clientWidth,
          scrollWidth: document.documentElement.scrollWidth,
        }));
        expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.width + 1);
        await captureLayoutThemes(page, process.env.PWA104_PROOF_DIR,
          `${route.slice(1).replaceAll("/", "-")}-${viewport.name}`);
        if (route.endsWith("/new")) {
          const cancel = main.getByRole("button", { name: /^(cancel|отмена)$/i });
          await cancel.scrollIntoViewIfNeeded();
          await expect(cancel).toBeInViewport({ ratio: 1 });
          await cancel.click();
          await expect(page).not.toHaveURL(new RegExp(`${route}$`));
        }
      });
    }

  });
}
