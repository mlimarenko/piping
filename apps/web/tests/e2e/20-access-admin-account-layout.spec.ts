import { captureLayoutThemes, expect, expectRoutedPage, test } from "./fixtures";

const routes = [
  "/access-control",
  "/access-control?tab=directory&section=users",
  "/access-control?tab=directory&section=groups",
  "/access-control?tab=directory&section=roles",
  "/access-control?tab=directory&section=grants",
  "/access-control?tab=sso",
  "/audit-logs",
  "/mcp",
  "/administration/license",
  "/administration/webhooks",
  "/administration/branding",
  "/administration/settings",
  "/administration/backup",
  "/user/settings",
  "/user/settings/credentials/new",
  "/user/settings/tokens/new",
  "/api-docs",
] as const;

const unavailableRoutes = [
  "/connections/config/00000000-0000-0000-0000-000000000000",
  "/connections/config/00000000-0000-0000-0000-000000000000/edit",
  "/connections/proxies/00000000-0000-0000-0000-000000000000",
  "/connections/proxies/00000000-0000-0000-0000-000000000000/edit",
  "/user/settings/credentials/00000000-0000-0000-0000-000000000000/edit",
  ...["sources", "metrics", "sets", "alert-sources"].flatMap(family => [
    `/observability/${family}/00000000-0000-0000-0000-000000000000`,
    `/observability/${family}/00000000-0000-0000-0000-000000000000/edit`,
  ]),
] as const;

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test.describe(`access, administration and account at ${viewport.name}`, () => {
    for (const route of routes) {
      test(`${route} keeps readable content and reachable actions`, async ({ appPage: page }) => {
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
        await captureLayoutThemes(page, process.env.PWA106_PROOF_DIR,
          `${route.slice(1).replaceAll(/[/?=&]/g, "-")}-${viewport.name}`);
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
    for (const route of unavailableRoutes) {
      test(`${route} keeps heading and navigation for a missing record`, async ({ appPage: page }) => {
        await page.setViewportSize(viewport);
        await page.goto(route);
        await expectRoutedPage(page);
        const main = page.locator("#main-content");
        await expect(main.getByText(/not found|не найден|не найдена|не найдены|failed to load|не удалось загрузить/i).first()).toBeVisible();
        await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 })).toHaveCount(1);
        await expect(main.getByRole("heading", { level: 1 })).toHaveCount(0);
        const back = page.getByRole("link", { name: /back|назад/i })
          .or(page.getByRole("button", { name: /back|назад|cancel|отмена/i })).first();
        await back.scrollIntoViewIfNeeded();
        await expect(back).toBeInViewport({ ratio: 1 });
        await back.click();
        await expect(page).not.toHaveURL(new RegExp(`${route}$`));
      });
    }
    test("inventory resource picker searches and selects with the keyboard", async ({ appPage: page }) => {
      await page.setViewportSize(viewport);
      await page.goto("/inventory/resources?entity=Sites");
      await expectRoutedPage(page);
      const picker = page.getByRole("combobox", { name: /DCIM/i });
      await expect(picker).toContainText(/sites|площадк/i);
      await picker.click();
      const search = page.getByPlaceholder(/^(search|поиск)$/i);
      await search.fill("Locations");
      const option = page.getByRole("option", { name: /locations|локаци/i });
      await expect(option).toBeVisible();
      await expect(option).toHaveAttribute("aria-selected", "true");
      await search.press("Enter");
      await expect(page).toHaveURL(/entity=Locations/);
      await expect(picker).toHaveAttribute("aria-expanded", "false");
      await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 })).toHaveText(/locations|локаци/i);
      await captureLayoutThemes(page, process.env.PWA106_PROOF_DIR, `inventory-search-picker-${viewport.name}`);
    });
    for (const editor of [
      { section: "users", action: /add user|добавить пользователя/i },
      { section: "groups", action: /add group|добавить группу/i },
      { section: "roles", action: /add role|добавить роль/i },
      { section: "policies", action: /new rule|новое правило/i },
      { section: "webhooks", action: /add webhook|добавить вебхук/i },
    ]) {
      test(`${editor.section} authoring keeps a reachable footer and restores focus`, async ({ appPage: page }) => {
        await page.setViewportSize(viewport);
        await page.goto(editor.section === "policies" ? "/access-control"
          : editor.section === "webhooks" ? "/administration/webhooks"
            : `/access-control?tab=directory&section=${editor.section}`);
        await expectRoutedPage(page);
        const open = page.getByRole("button", { name: editor.action }).first();
        await open.click();
        const host = page.locator("[data-entity-editor-host]");
        const editorHeader = host.locator('[data-slot="entity-editor-page"] > header');
        await expect(editorHeader).toHaveCount(1);
        await expect(editorHeader.getByRole("heading", { level: 2 })).toHaveCount(1);
        await expect(editorHeader.getByRole("heading", { level: 2 })).toBeVisible();
        await expect(host.getByRole("textbox").first()).toBeVisible();
        await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
        await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 })).toHaveCount(1);
        const cancel = host.locator("footer").getByRole("button", { name: /^(cancel|отмена)$/i });
        await cancel.scrollIntoViewIfNeeded();
        await expect(cancel).toBeInViewport({ ratio: 1 });
        expect((await cancel.boundingBox())?.height).toBe(36);
        await captureLayoutThemes(page, process.env.PWA106_PROOF_DIR, `access-${editor.section}-editor-${viewport.name}`);
        await cancel.click();
        await expect(host).toHaveCount(0);
        await expect(open).toBeFocused();
      });
    }
  });
}
