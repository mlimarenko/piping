import { apiRequest, expect, test, expectRoutedPage } from "./fixtures";

test.describe("playbooks UX", () => {
  test("keeps setup pages focused on the current resource", async ({
    appPage: page,
  }) => {
    await page.goto("/automation/execution-environments");
    await page.waitForLoadState("networkidle");
    await expectRoutedPage(page);

    await expect(page.getByTestId("playbooks-workflow")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /New playbook|Новый плейбук/i })).toHaveCount(0);
    await expect(page.getByRole("button", { name: /Run playbook|Запустить плейбук/i })).toHaveCount(0);
    await expect(
      page.getByRole("button", {
        name: /New execution environment|Новая среда выполнения/i,
      }),
    ).toBeVisible();

    const setupNavigation = page.getByRole("navigation", {
      name: /^Setup$|^Настройка$/i,
    });
    const executionEnvironmentsLink = setupNavigation.getByRole("link", {
      name: /^Execution Environments$|^Среды выполнения$/i,
    });
    const targetSetsLink = setupNavigation.getByRole("link", {
      name: /^Target Sets$|^Наборы целей$/i,
    });
    await expect(executionEnvironmentsLink).toHaveAttribute("aria-current", "page");
    await expect(targetSetsLink).toHaveAttribute("href", /\/automation\/target-sets$/);
    await expect(targetSetsLink).not.toHaveAttribute("aria-current", "page");
    await targetSetsLink.focus();
    await targetSetsLink.press("Enter");
    await expect(page).toHaveURL(/\/automation\/target-sets$/);
    await expectRoutedPage(page);
    await expect(page.getByRole("heading", { name: /Target sets|Наборы целей/i })).toBeVisible();
    await expect(targetSetsLink).toHaveAttribute("aria-current", "page");
    await expect(executionEnvironmentsLink).not.toHaveAttribute("aria-current", "page");
    await expect(page.getByTestId("playbooks-workflow")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /Run playbook|Запустить плейбук/i })).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: /New target set|Новый набор целей/i }),
    ).toBeVisible();
  });

  test("opens execution environment details from the row", async ({
    appPage: page,
  }) => {
    await page.goto("/automation/execution-environments");
    await page.waitForLoadState("networkidle");
    await expectRoutedPage(page);

    const row = page.locator("tr", { hasText: "Default Ansible" }).first();
    await expect(row).toBeVisible();
    await expect(row).toHaveAttribute("role", "button");
    const actions = row.getByRole("button", { name: /Actions: Default Ansible|Действия: Default Ansible/i });
    await expect(actions).toBeVisible();

    await row.focus();
    await row.press("Enter");

    const dialog = page.getByRole("dialog", { name: "Default Ansible" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("piping-ansible-runner:local")).toBeVisible();

    await dialog.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(row).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(actions).toBeFocused();
    await actions.click();
    await expect(dialog).toHaveCount(0);
    await page.getByRole("menuitem", { name: /^(Details|Детали)$/i }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByText("piping-ansible-runner:local")).toBeVisible();
    await dialog.getByRole("button", { name: /^(Close|Закрыть)$/i }).last().click();
    await expect(actions).toBeFocused();
  });

  test("keeps a stable collection anchor when a refetch removes the preview row", async ({ appPage: page }) => {
    await page.goto("/automation/execution-environments?lang=en");
    const row = page.locator("tr", { hasText: "Default Ansible" });
    await expect(row).toBeVisible();
    const response = await apiRequest(page, "GET", "/api/v1/playbooks/execution-environments");
    const data = await response.json() as { items: { id: string }[] };
    expect(data.items).toHaveLength(1);
    const created = await apiRequest(page, "POST", "/api/v1/playbooks/execution-environments", { data: {
      name: "Surviving runner", image: "piping-ansible-runner:local", enabled: true,
    } });
    expect(created.ok()).toBe(true);
    await row.focus();
    await row.press("Enter");
    const dialog = page.getByRole("dialog", { name: "Default Ansible" });
    await expect(dialog).toBeVisible();
    const environment = data.items[0];
    if (!environment) throw new Error("Demo environment fixture is missing");
    const removed = await apiRequest(page, "DELETE", `/api/v1/playbooks/execution-environments/${environment.id}`);
    expect(removed.ok()).toBe(true);
    await page.getByRole("button", { name: "Refresh", includeHidden: true }).evaluate(button => {
      if (!(button instanceof HTMLButtonElement)) throw new Error("Refresh must be a button");
      button.click();
    });
    await expect(row).toHaveCount(0);
    await expect(dialog).toBeVisible();
    await dialog.press("Escape");
    await expect(page.getByRole("region", { name: "Execution environments", exact: true })).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(page.locator(":focus")).not.toHaveText(/Skip to content/);
  });
});
