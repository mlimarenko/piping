import { apiRequest, expect, test, expectRoutedPage, openActionsMenu } from "./fixtures";

/**
 * Inventory Sites CRUD smoke — covers the happy path through
     * the routed Sites entity editor.
 *
 * The test creates a row with a timestamped name so re-runs don't collide,
 * then deletes it. Creation uses its editor page, deletion a confirmation dialog,
 * not the API, because the point is to exercise the UI wiring.
 */
test.describe("inventory Sites CRUD", () => {
  test("create then delete a site", async ({ appPage: page }) => {
    const siteName = `e2e-test-site-${Date.now()}`;

    await page.goto("/inventory/resources");
    await page.waitForLoadState("networkidle");
    await expectRoutedPage(page);

    await page.getByRole("combobox", { name: /DCIM/i }).click();
    await page.getByRole("option", { name: /^(sites|площадк)/i }).click();

    // Use the catalogue's primary action rather than an inline related picker.
    await page.getByRole("button", { name: /^(create|создать)/i }).first().click();

    await expect(page).toHaveURL(/\/inventory\/resources\/Sites\/new/);
    const editor = page.getByRole("main");

    await editor.getByPlaceholder(/enter name|введите имя/i).first().fill(siteName);

    // Match the primary action exactly, not a nested picker's create action.
    await editor
      .getByRole("button", { name: /^(save|сохранить|create|создать)$/i })
      .click();

    // A successful create returns to the catalogue without a full reload.
    await expect(page).toHaveURL(/\/inventory\/resources(?:\?|$)/, { timeout: 10_000 });

    // Row visible in the table. Search narrows the list so the assertion is
    // deterministic even when the list already has many sites. Unconditional:
    // this used to be wrapped in `if (await searchBox.isVisible())`, which
    // silently changed what the rest of the test asserted if the control ever
    // disappeared.
    const searchBox = page.getByPlaceholder(/search|поиск/i).first();
    await expect(searchBox).toBeVisible();
    await searchBox.fill(siteName);
    const row = page.getByRole("row", { name: new RegExp(siteName, "i") });
    await expect(row).toBeVisible({ timeout: 10_000 });

    // Delete through the row menu, then confirm in AlertDialog.
    const menu = await openActionsMenu(page, row.getByRole("button", { name: /^(actions|действия):/i }));
    await menu.getByRole("menuitem", { name: /^(delete|удалить)$/i }).click();
    const confirm = page.getByRole("alertdialog");
    await expect(confirm).toBeVisible();
    await confirm.getByRole("button", { name: /delete|удалить|confirm|подтверд/i }).click();

    await expect(row).toBeHidden({ timeout: 10_000 });

    // The row leaving the table is not proof the site was deleted — the list is
    // filtered and re-rendered client-side, and a DELETE that changed nothing
    // still empties this view. Read the collection back in the same page
    // session (a reload would re-seed the demo store and hide the difference).
    const listing = await apiRequest(page, "GET", "/api/v1/inventory/sites", {
      params: { limit: 1000, offset: 0 },
    });
    expect(listing.ok(), await listing.text()).toBeTruthy();
    const { items } = (await listing.json()) as { items: { name: string }[] };
    expect(items.map((item) => item.name)).not.toContain(siteName);
  });
});
