import { expect, expectRoutedPage, test } from "./fixtures";
import en from "../../src/shared/i18n/locales/en";
import ru from "../../src/shared/i18n/locales/ru";

for (const scenario of [
  { language: "en", width: 1440, height: 900 },
  { language: "ru", width: 390, height: 844 },
] as const) {
  test(`inventory target repair ${scenario.language} ${scenario.width}`, async ({ page }, info) => {
    const strings = scenario.language === "en" ? en : ru;
    await page.setViewportSize(scenario);
    await page.addInitScript(language => {
      Object.defineProperty(navigator, "language", { value: language });
      Object.defineProperty(navigator, "languages", { value: [language] });
      const original = window.fetch.bind(window);
      Object.assign(window, { inventoryFixtureMode: "empty", inventoryFixtureQueries: [] });
      window.fetch = async (input, init) => {
        const url = new URL(input instanceof Request ? input.url : String(input), location.href);
        const fixture = window as unknown as { inventoryFixtureMode: string; inventoryFixtureQueries: Record<string, string>[] };
        if (url.origin !== location.origin) return original(input, init);
        if (url.pathname === "/api/v1/auth/me") {
          const response = await original(input, init);
          return new Response(JSON.stringify({ ...await response.json(), roles: [], capabilities: [{ action: "READ", resource_type: "INVENTORY" }] }), { headers: { "Content-Type": "application/json" } });
        }
        if (url.pathname === "/api/v1/inventory/devices") {
          fixture.inventoryFixtureQueries.push(Object.fromEntries(url.searchParams));
          const empty = fixture.inventoryFixtureMode === "empty" || Boolean(url.searchParams.get("status")) || Boolean(url.searchParams.get("search"));
          return new Response(JSON.stringify({ items: empty ? [] : [{ id: "fixture-device", name: "Fixture device", status: "active" }], total: fixture.inventoryFixtureMode === "empty" ? 0 : 120, limit: Number(url.searchParams.get("limit") ?? 50), offset: Number(url.searchParams.get("offset") ?? 0) }), { headers: { "Content-Type": "application/json" } });
        }
        return original(input, init);
      };
    }, scenario.language);
    await page.goto("/inventory/resources?entity=Devices");
    await expectRoutedPage(page);
    await expect(page.getByText(strings.inventory.resources.noAvailableRecords, { exact: true })).toBeVisible();
    await expect(page.getByText(strings.inventory.resources.createEntry.replace("{{entity}}", strings.inventory.resources.entities.Devices.one), { exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: strings.create, exact: true })).toHaveCount(0);
    await page.screenshot({ path: info.outputPath("readonly-empty.png"), fullPage: true });
    await page.evaluate(() => { (window as unknown as { inventoryFixtureMode: string }).inventoryFixtureMode = "rows"; });
    await page.getByRole("button", { name: strings.refresh, exact: true }).click();
    await expect(page.getByText("Fixture device", { exact: true }).first()).toBeVisible();
    const search = page.getByPlaceholder(strings.inventory.resources.searchPlaceholder.replace("{{entity}}", strings.inventory.resources.entities.Devices.other));
    await page.getByRole("combobox", { name: strings.pagination.perPageLabel }).selectOption("100");
    await page.locator('[data-column-sort-key="name"]').click();
    // Column settings operate on the real persisted table preference owner.
    await page.getByRole("button", { name: strings.inventory.resources.columnSettings, exact: true }).click();
    const platform = page.getByRole("menuitemcheckbox", { name: /^(Platform|Платформа)$/ });
    const platformWasVisible = await platform.getAttribute("aria-checked") === "true";
    await platform.click();
    await expect(platform).toHaveAttribute("aria-checked", String(!platformWasVisible));
    await page.keyboard.press("Escape");
    const platformVisible = platformWasVisible ? 0 : 1;
    await expect(page.getByRole("columnheader", { name: /Platform|Платформа/ })).toHaveCount(platformVisible);
    await search.fill("no-fixture-match");
    const reset = page.getByRole("button", { name: strings.inventory.resources.resetQuery, exact: true });
    await expect(reset).toBeVisible();
    await page.getByRole("button", { name: strings.pagination.next, exact: true }).click();
    await expect.poll(() => page.evaluate(() => {
      const queries = (window as unknown as { inventoryFixtureQueries: Record<string, string>[] }).inventoryFixtureQueries;
      return queries[queries.length - 1]?.offset;
    })).toBe("100");
    await reset.focus();
    await reset.press("Enter");
    await expect(search).toBeVisible();
    await expect(search).toBeFocused();
    await expect(search).toHaveValue("");
    await expect(reset).toHaveCount(0);
    await expect(page.getByText("Fixture device", { exact: true }).first()).toBeVisible();
    await expect(search).toBeFocused();
    await expect(page.getByRole("button", { name: strings.pagination.goToPage.replace("{{page}}", "1"), exact: true })).toHaveAttribute("aria-current", "page");
    await page.screenshot({ path: info.outputPath("reset-search-focus.png"), fullPage: true });
    // A fresh page-zero result may be served from the application's query cache.
    // Refresh observes the current query dimensions without changing them.
    await page.getByRole("button", { name: strings.refresh, exact: true }).click();
    await expect.poll(() => page.evaluate(() => {
      const queries = (window as unknown as { inventoryFixtureQueries: Record<string, string>[] }).inventoryFixtureQueries;
      return queries[queries.length - 1]?.offset;
    })).toBe("0");
    const query = await page.evaluate(() => {
      const queries = (window as unknown as { inventoryFixtureQueries: Record<string, string>[] }).inventoryFixtureQueries;
      return queries[queries.length - 1];
    });
    expect(query).toMatchObject({ limit: "100", offset: "0", sort: "name", order: "asc" });
    if (!query) throw new Error("Reset did not issue an inventory query");
    expect(query.search).toBeUndefined();
    expect(query.status).toBeUndefined();
    expect(await page.getByRole("columnheader", { name: /Platform|Платформа/ }).count()).toBe(platformVisible);
    await expect(page.getByRole("combobox", { name: strings.pagination.perPageLabel })).toHaveValue("100");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}

for (const language of ["en", "ru"] as const) {
  for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
    test(`password and inventory feedback ${language} ${viewport.width}`, async ({ page }, testInfo) => {
      testInfo.setTimeout(60_000);
      await page.setViewportSize(viewport);
      await page.addInitScript(language => {
        Object.defineProperty(navigator, "language", { value: language });
        Object.defineProperty(navigator, "languages", { value: [language] });
        const original = window.fetch.bind(window);
        let attempt = 0;
        window.fetch = async (input, init) => {
          const url = new URL(input instanceof Request ? input.url : String(input), location.href);
          if (url.origin !== location.origin) return original(input, init);
          if (url.pathname === "/api/v1/auth/me") {
            const response = await original(input, init);
            return new Response(JSON.stringify({ ...await response.json(), auth_source: "local" }), { headers: { "Content-Type": "application/json" } });
          }
          if (url.pathname === "/api/v1/user/settings/password") {
            attempt++;
            // The fixture controls completion without a timer or real credentials.
            await new Promise<void>(resolve => { Object.assign(window, { finishPasswordFixture: resolve }); });
            if (attempt <= 2) return new Response(JSON.stringify({ code: "VALIDATION_ERROR", message: "internal fixture diagnostic", trace_id: "fixture-trace" }), { status: attempt === 1 ? 400 : 500, headers: { "Content-Type": "application/json" } });
            return new Response(JSON.stringify({ user_id: "fixture-user", password_changed_at: "2026-10-13T00:00:00Z" }), { headers: { "Content-Type": "application/json" } });
          }
          if (url.pathname === "/api/v1/inventory/devices" && (url.searchParams.has("status") || url.searchParams.has("search"))) {
            return new Response(JSON.stringify({ items: [], total: 0, limit: 25, offset: 0 }), { headers: { "Content-Type": "application/json" } });
          }
          return original(input, init);
        };
      }, language);
      const strings = language === "en" ? en : ru;
      await page.goto("/user/settings");
      await expectRoutedPage(page);
      const current = page.locator("#account-current-password");
      const next = page.locator("#account-new-password");
      const confirm = page.locator("#account-confirm-password");
      const submit = page.getByRole("button", { name: strings.userSettings.changePassword, exact: true });
      await submit.click();
      await expect(current).toBeFocused();
      await expect(current).toHaveAttribute("aria-invalid", "true");
      await expect(current).toHaveAccessibleDescription(strings.userSettings.currentPasswordRequired);
      await current.fill("synthetic-current");
      await next.fill("short");
      await confirm.press("Enter");
      await expect(next).toBeFocused();
      await expect(next).toHaveAccessibleDescription(strings.userSettings.passwordTooShort);
      await next.fill("synthetic-next");
      await confirm.fill("synthetic-different");
      await confirm.press("Enter");
      await expect(confirm).toBeFocused();
      await expect(confirm).toHaveAccessibleDescription(strings.userSettings.passwordsDontMatch);
      await page.screenshot({ path: testInfo.outputPath("password-invalid.png"), fullPage: true });
      await confirm.fill("synthetic-next");
      for (let attempt = 0; attempt < 3; attempt++) {
        await confirm.press("Enter");
        await expect(page.getByRole("button", { name: strings.userSettings.passwordSaving })).toBeDisabled();
        await expect(current).toBeDisabled();
        await page.evaluate(() => (window as unknown as { finishPasswordFixture: () => void }).finishPasswordFixture());
        if (attempt < 2) {
          await expect(page.getByRole("alert").filter({ hasText: strings.userSettings.passwordChangeFailed })).toBeVisible();
          await expect(page.getByText("internal fixture diagnostic")).toHaveCount(0);
          // FormFieldControl removes aria-invalid when the field error clears.
          await expect(current).not.toHaveAttribute("aria-invalid", "true");
          await expect(submit).toBeEnabled();
        } else {
          await expect(current).toHaveValue("");
          await expect(next).toHaveValue("");
          await expect(confirm).toHaveValue("");
        }
      }
      await expect(page.locator("footer")).toContainText(process.env.PWA116_EXPECT_VERSION ?? "dev");
      await page.goto("/inventory/resources?entity=Devices");
      await expectRoutedPage(page);
      const search = page.getByPlaceholder(strings.inventory.resources.searchPlaceholder.replace("{{entity}}", strings.inventory.resources.entities.Devices.other));
      await search.fill("no-fixture-match");
      await expect(page.getByText(strings.noResults, { exact: true })).toBeVisible();
      const reset = page.getByRole("button", { name: strings.inventory.resources.resetQuery, exact: true });
      await expect(reset).toBeVisible();
      await reset.focus();
      await expect(reset).toBeFocused();
      await page.screenshot({ path: testInfo.outputPath("inventory-no-match.png"), fullPage: true });
      await reset.press("Enter");
      await expect(search).toHaveValue("");
      await expect(reset).toHaveCount(0);
      await expect(page.getByRole("heading", { level: 1, name: strings.inventory.resources.entities.Devices.other })).toBeVisible();
      if (viewport.width >= 768) {
        await page.getByRole("button", { name: /(?:filter|фильтр):?\s*(status|статус)/i }).click();
        await page.getByRole("button", { name: language === "en" ? "Planned" : "Планируется", exact: true }).click();
        await page.keyboard.press("Escape");
        await expect(page.getByText(strings.noResults, { exact: true })).toBeVisible();
        await reset.click();
        await expect(reset).toHaveCount(0);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    });
  }
}

test("inventory target repair authorized empty creation", async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "language", { value: "en" });
    Object.defineProperty(navigator, "languages", { value: ["en"] });
    const original = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      if (url.origin === location.origin && url.pathname === "/api/v1/inventory/devices") {
        return Promise.resolve(new Response(JSON.stringify({ items: [], total: 0, limit: 50, offset: 0 }), { headers: { "Content-Type": "application/json" } }));
      }
      return original(input, init);
    };
  });
  await page.goto("/inventory/resources?entity=Devices");
  await expectRoutedPage(page);
  await expect(page.getByText("Create your first Device entry", { exact: true })).toBeVisible();
  await page.screenshot({ path: info.outputPath("authorized-empty.png"), fullPage: true });
  await page.getByRole("button", { name: "Add Device", exact: true }).click();
  await expect(page).toHaveURL(/\/inventory\/resources\/[^/]+\/new\?/);
  await expectRoutedPage(page);
});
