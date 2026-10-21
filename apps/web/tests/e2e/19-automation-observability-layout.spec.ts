import { captureLayoutThemes, expect, expectRoutedPage, test } from "./fixtures";

const routes = [
  "/playbooks",
  "/automation/projects",
  "/automation/job-templates",
  "/automation/execution-environments",
  "/automation/assignments",
  "/automation/target-sets",
  "/automation/schedules",
  "/automation/service-principals",
  "/automation/runs",
  "/automation/secrets",
  "/observability",
  "/observability/sources/new",
  "/observability/metrics/new",
  "/observability/sets/new",
  "/observability/alert-sources/new",
] as const;

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test.describe(`automation and observability at ${viewport.name}`, () => {
    for (const route of routes) {
      test(`${route} exposes one readable page`, async ({ appPage: page }) => {
        await page.setViewportSize(viewport);
        await page.goto(route);
        await expectRoutedPage(page);
        const main = page.locator("#main-content");
        if (route.endsWith("/new")) await expect(main.getByRole("textbox").first()).toBeVisible();
        await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 })).toHaveCount(1);
        await expect(main.getByRole("heading", { level: 1 })).toHaveCount(0);
        const geometry = await page.evaluate(() => ({
          width: document.documentElement.clientWidth,
          scrollWidth: document.documentElement.scrollWidth,
        }));
        expect(geometry.scrollWidth).toBeLessThanOrEqual(geometry.width + 1);
        await captureLayoutThemes(page, process.env.PWA105_PROOF_DIR,
          `${route.slice(1).replaceAll("/", "-")}-${viewport.name}`);
        if (route.endsWith("/new")) {
          const cancel = main.getByRole("button", { name: /^(cancel|отмена)$/i });
          await cancel.scrollIntoViewIfNeeded();
          await expect(cancel).toBeInViewport({ ratio: 1 });
          expect((await cancel.boundingBox())?.height).toBe(36);
          await cancel.click();
          await expect(page).not.toHaveURL(new RegExp(`${route}$`));
        }
      });
    }
  });
}
