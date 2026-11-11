import { apiRequest, expect, test, expectRoutedPage, openActionsMenu } from "./fixtures";

test.describe("connection-config CRUD", () => {
  test("creates, edits and deletes a connection configuration through its pages", async ({ appPage: page }) => {
    const recordName = `e2e-connconf-${Date.now()}`;
    await page.goto("/connections/config");
    await expectRoutedPage(page);
    await page.getByRole("link", { name: /create configuration|создать конфигурацию/i }).click();
    await expect(page).toHaveURL(/\/connections\/config\/new$/);
    await page.getByRole("textbox", { name: /^Name|^Название/i }).fill(recordName);
    await expect(page.getByRole("textbox", { name: /^Name|^Название/i })).toHaveValue(recordName);
    await page.getByRole("button", { name: /create configuration|создать конфигурацию/i }).click();
    await expect(page).toHaveURL(/\/connections\/config\/[^/]+\?tab=assignments$/, { timeout: 10_000 });

    const header = page.locator('[data-slot="app-header"]');
    const actions = header.getByRole("button", { name: /^(actions|действия):/i });
    const editMenu = await openActionsMenu(page, actions);
    await editMenu.getByRole("menuitem", { name: /^(edit|изменить|редактировать)$/i }).click();
    await expect(page).toHaveURL(/\/connections\/config\/[^/]+\/edit$/);
    await page.getByRole("textbox", { name: /^Name|^Название/i }).fill(`${recordName}-edited`);
    await page.getByRole("button", { name: /save changes|сохранить изменения/i }).click();
    await expect(page).toHaveURL(/\/connections\/config\/[^/]+$/, { timeout: 10_000 });
    await expect(page.getByRole("heading", { name: `${recordName}-edited`, exact: true })).toBeVisible();
    const readback = await apiRequest(page, "GET", "/api/v1/connection-config/records", { params: { limit: 1000, offset: 0 } });
    expect(readback.ok(), await readback.text()).toBeTruthy();
    const created = await readback.json() as { items: { name: string }[] };
    expect(created.items.map(item => item.name)).toContain(`${recordName}-edited`);

    const deleteMenu = await openActionsMenu(page, actions);
    await deleteMenu.getByRole("menuitem", { name: /^(delete|удалить)$/i }).click();
    const confirm = page.getByRole("alertdialog");
    await expect(confirm).toBeVisible();
    await confirm.getByRole("button", { name: /^(delete|удалить)$/i }).click();
    await expect(page).toHaveURL(/\/connections\/config$/, { timeout: 10_000 });
    const listing = await apiRequest(page, "GET", "/api/v1/connection-config/records", { params: { limit: 1000, offset: 0 } });
    expect(listing.ok(), await listing.text()).toBeTruthy();
    const remaining = await listing.json() as { items: { name: string }[] };
    const names = remaining.items.map(item => item.name);
    expect(names).not.toContain(recordName);
    expect(names).not.toContain(`${recordName}-edited`);
  });
});
