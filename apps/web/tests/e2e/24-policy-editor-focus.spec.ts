import { expect, expectRoutedPage, test } from "./fixtures";

for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
  test(`policy row menu returns focus on Back at ${viewport.width}px without API writes`, async ({ appPage: page }) => {
    const writes: string[] = [];
    await page.setViewportSize(viewport);
    await page.goto("/access-control");
    await expectRoutedPage(page);
    const trigger = page.getByRole("button", { name: /^(actions|действия): Allow All Inventory$/i });
    await expect(trigger).toBeVisible();
    // Measure editor actions after initial authentication/session refresh.
    page.on("request", request => {
      if (new URL(request.url()).pathname.startsWith("/api/v1/") && !["GET", "HEAD", "OPTIONS"].includes(request.method())) {
        writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
      }
    });
    await trigger.click();
    await page.getByRole("menuitem", { name: /^(edit|редактировать)$/i }).click();
    const editor = page.getByTestId("entity-editor-page");
    await expect(editor.getByRole("heading", { level: 2, name: /^(edit policy|редактировать политику)$/i })).toBeFocused();
    await expect(page.getByRole("menu")).toHaveCount(0);
    await editor.getByRole("button", { name: /^(back|назад)$/i }).click();
    await expect(editor).toHaveCount(0);
    await expect(trigger).toBeFocused();
    expect(await trigger.evaluate(element => element.closest("[inert]") === null)).toBe(true);
    expect(writes).toEqual([]);
  });
}
