import { captureLayoutThemes, expect, expectRoutedPage, test } from "./fixtures";

test("whitespace and padded searches retain loaded targets across catalogue views", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const url = new URL(input instanceof Request ? input.url : String(input), window.location.href);
      if (url.origin === window.location.origin && url.pathname === "/api/v1/connections/targets") {
        const query = url.searchParams.get("search") || "";
        const items = query === "" || query === "router-1"
          ? [{ id: "00000000-0000-4000-8000-000000000001", name: "router-1", target_kind: "device", status: "active" }]
          : [];
        return Promise.resolve(new Response(JSON.stringify({ items, total: items.length, limit: 100, offset: 0 }), {
          headers: { "Content-Type": "application/json" },
        }));
      }
      return originalFetch(input, init);
    };
  });
  await page.goto("/connections?lang=en");
  await expectRoutedPage(page);
  for (const mode of ["Tree", "Cards", "Table"]) {
    await page.getByRole("button", { name: "Catalogue view and actions" }).click();
    await page.getByRole("menuitem", { name: mode, exact: true }).click();
    await expect(page.getByText("router-1", { exact: true }).first()).toBeVisible();
    const search = page.getByRole("textbox").first();
    for (const term of [" ", " router-1 "]) {
      await search.fill("missing-target");
      await expect(page.getByText("No targets match your search or filters", { exact: true })).toBeVisible();
      await search.fill(term);
      await expect(page.getByText("router-1", { exact: true }).first()).toBeVisible();
      await expect(search).toHaveValue(term);
      await expect(page.getByText("No connection targets available", { exact: true })).toHaveCount(0);
      await expect(page.getByRole("link", { name: "Add device" })).toHaveCount(0);
    }
    await search.fill("");
    await captureLayoutThemes(page, process.env.PWA114_SCREENSHOTS, `whitespace-${mode.toLowerCase()}`);
  }
});

for (const viewport of [
  { name: "desktop-en", width: 1440, height: 900, language: "en" },
  { name: "mobile-ru", width: 390, height: 844, language: "ru" },
]) {
  test(`catalogue errors, empty setup and existing editors: ${viewport.name}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await page.addInitScript(language => {
      localStorage.setItem("i18nextLng", language);
      const originalFetch = window.fetch.bind(window);
      let failed = true;
      document.addEventListener("click", event => {
        if ((event.target as HTMLElement).closest("button")?.textContent?.match(/Retry|Повторить/)) failed = false;
      }, true);
      window.fetch = (input, init) => {
        const url = new URL(input instanceof Request ? input.url : String(input), window.location.href);
        if (url.origin === window.location.origin && url.pathname === "/api/v1/connections/targets") {
          const status = failed ? (language === "ru" ? 403 : 500) : 200;
          return Promise.resolve(new Response(JSON.stringify(status !== 200
            ? { message: "private fixture detail" }
            : { items: [], total: 0, limit: 100, offset: 0 }), {
            status, headers: { "Content-Type": "application/json" },
          }));
        }
        return originalFetch(input, init);
      };
    }, viewport.language);
    await page.goto(`/connections?lang=${viewport.language}`);
    await expectRoutedPage(page);
    await expect(page.getByRole("alert")).toContainText(/Could not load connection targets|Не удалось загрузить цели подключений/);
    await captureLayoutThemes(page, process.env.PWA114_SCREENSHOTS, `${viewport.name}-error`);
    await page.getByRole("button", { name: /Retry|Повторить/ }).click();
    const empty = page.getByText(/No connection targets available|Нет доступных целей подключений/, { exact: true });
    await expect(empty).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("lang", viewport.language);
    await expect(page.getByText("private fixture detail")).toHaveCount(0);
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await captureLayoutThemes(page, process.env.PWA114_SCREENSHOTS, viewport.name);
    for (const mode of [/Table|Таблица/, /Cards|Карточки/, /Tree|Дерево/]) {
      await page.getByRole("button", { name: /Catalogue view and actions|Вид и действия каталога/ }).click();
      await page.getByRole("menuitem", { name: mode, exact: true }).click();
      await expect(empty).toBeVisible();
      const search = page.getByRole("textbox").first();
      await search.fill("missing fixture target");
      await expect(page.getByText(/No targets match your search or filters|Нет целей по вашему поиску или фильтрам/, { exact: true })).toBeVisible();
      await page.getByRole("button", { name: /Reset search and filters|Сбросить поиск и фильтры/ }).click();
      await expect(search).toBeFocused();
      await expect(search).toHaveValue("");
      await expect(empty).toBeVisible();
    }
    for (const name of [/Add device|Добавить устройство/, /Add virtual machine|Добавить виртуальную машину/]) {
      const link = page.getByRole("link", { name });
      await expect(link).toBeVisible();
      await link.focus();
      await expect(link).toBeFocused();
      await page.keyboard.press("Enter");
      await expect(page).toHaveURL(/\/inventory\/resources\/.+\/new\?return=%2Fconnections/);
      await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
      await page.getByRole("button", { name: /Back to connections|К подключениям/ }).click();
      await expect(page).toHaveURL(/\/connections$/);
      await expect(empty).toBeVisible();
    }
  });
}
