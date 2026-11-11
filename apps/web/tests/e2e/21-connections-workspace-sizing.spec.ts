import { captureLayoutThemes, expect, expectRoutedPage, test } from "./fixtures";

for (const width of [1440, 2000]) {
  test(`session width is user-controlled and table columns remain reachable at ${width}px`, async ({ appPage: page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/connections");
    await expectRoutedPage(page);
    const connect = page.getByTestId(/^quick-access-/).first();
    await connect.click();
    await page.getByRole("menuitem").filter({ hasText: /SSH/ }).first().click();
    const workspace = page.getByTestId("session-workspace");
    await expect(workspace).toBeVisible();
    const dockRight = workspace.getByRole("button", { name: /dock.*right|справа/i });
    if (await dockRight.count()) await dockRight.first().click();
    const resize = workspace.getByRole("slider", { name: /session panel size|размер панели сессий/i });
    await expect(resize).toBeVisible();
    const viewer = workspace.locator("[data-session-pane-viewer]").first();
    const node = await viewer.elementHandle();
    await resize.press("Home");
    await expect(resize).toHaveAttribute("aria-valuenow", "320");
    await expect.poll(async () => Math.round((await workspace.boundingBox())!.width)).toBe(320);
    expect(await viewer.evaluate((element, original) => element === original, node)).toBe(true);
    await resize.press("ArrowLeft");
    await expect.poll(async () => Math.round((await workspace.boundingBox())!.width)).toBe(340);
    await captureLayoutThemes(page, process.env.PWA108_PROOF_DIR, `tree-with-small-session-${width}`);

    await page.getByRole("button", { name: /^web-prod-01/ }).first().click();
    const inspector = page.getByRole("complementary", { name: /selected target details|сведения о выбранной цели/i });
    await expect(inspector).toBeVisible();
    const pin = inspector.getByRole("button", { name: /^pin details$|^закрепить сведения$/i });
    if (await pin.count()) await pin.click();
    await resize.press("End");
    const catalogueSearch = page.getByPlaceholder(/search targets|поиск целей/i);
    await expect.poll(async () => (await catalogueSearch.boundingBox())!.width).toBeGreaterThan(120);
    await expect.poll(async () => {
      const session = (await workspace.boundingBox())!;
      return session.x - (await page.locator("#main-content").boundingBox())!.x - 12;
    }).toBeGreaterThanOrEqual(239.5);
    await resize.press("Home");

    await page.getByRole("button", { name: /catalogue view and actions|вид.*действия.*каталог/i }).click();
    await page.getByRole("menuitem", { name: /^(table|таблица)$/i }).click();
    const table = page.locator('[data-table-id="connections.targets.desktop"]');
    await expect(table.getByRole("columnheader", { name: /^name|^имя/i })).toBeVisible();
    await expect(table.getByRole("columnheader", { name: /^status|^статус/i })).toBeInViewport();
    const scroll = table.locator("[data-table-scroll-viewport]");
    await scroll.evaluate(element => { element.scrollLeft = element.scrollWidth; });
    await expect(table.getByRole("columnheader", { name: /primary ip|основной ip/i })).toBeInViewport();
    await captureLayoutThemes(page, process.env.PWA108_PROOF_DIR, `table-with-small-session-${width}`);
    await workspace.getByRole("button", { name: /session tools|инструменты сессии/i }).first().click();
    await page.getByRole("menuitem", { name: /dock.*bottom|снизу/i }).click();
    await resize.press("Home");
    await expect(resize).toHaveAttribute("aria-valuenow", "140");
    await expect.poll(async () => Math.round((await workspace.boundingBox())!.height)).toBe(140);
    expect(await viewer.evaluate((element, original) => element === original, node)).toBe(true);
  });
}

test("compact tree launch controls remain accessible on a phone", async ({ appPage: page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/connections");
  await expectRoutedPage(page);
  await page.getByRole("button", { name: /catalogue view and actions|вид.*действия.*каталог/i }).click();
  await page.getByRole("menuitem", { name: /^(tree|дерево)$/i }).click();
  await expect(page.getByRole("menu")).toBeHidden();
  await expect(page.getByTestId(/^quick-access-/).first()).toBeVisible();
  await page.getByTestId(/^quick-access-/).first().click();
  await expect(page.getByRole("menu")).toBeVisible();
  await page.getByRole("menuitem").first().focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toBeHidden();
  await captureLayoutThemes(page, process.env.PWA108_PROOF_DIR, "tree-mobile");
});
