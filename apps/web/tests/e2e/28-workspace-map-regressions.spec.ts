import { expect, test } from "./fixtures";
import type { Locator, Page } from "@playwright/test";

function targetInspector(page: Page, name: string) {
  return page.getByRole("complementary", { name, exact: true }).or(page.getByRole("dialog", { name, exact: true }));
}

async function expectInspectorForeground(inspector: Locator) {
  // A Sheet becomes visible before its entry animation reaches the viewport.
  await expect.poll(() => inspector.evaluate(element => {
    const rect = element.getBoundingClientRect();
    return element.contains(document.elementFromPoint(rect.left + 48, rect.bottom - 64));
  })).toBe(true);
}

test("distinct Site geography survives target selection and Workspace search", async ({ appPage: page }) => {
  await page.goto("/connections?lang=en&dense-map=1");
  await page.getByRole("button", { name: "Map", exact: true }).click();
  const locations = page.getByRole("complementary", { name: "Targets by site" });
  const frankfurt = page.locator("svg").getByRole("button", { name: "Frankfurt DC", exact: true });
  const amsterdam = page.locator("svg").getByRole("button", { name: /Amsterdam Campus/ });
  await expect(page.locator("svg").getByRole("button", { name: "2 locations", exact: true })).toBeVisible();
  await expect(frankfurt).toHaveCount(0);
  await locations.getByRole("button", { name: /Frankfurt DC/ }).click();
  await expect(frankfurt).toBeVisible();
  await expect(amsterdam).toHaveCount(1);
  const frankfurtPosition = (await frankfurt.getAttribute("transform"))?.match(/translate\(([^ ]+) ([^)]+)\)/);
  const amsterdamPosition = (await amsterdam.getAttribute("transform"))?.match(/translate\(([^ ]+) ([^)]+)\)/);
  expect(frankfurtPosition).toBeTruthy();
  expect(amsterdamPosition).toBeTruthy();
  expect(frankfurtPosition?.[1]).not.toBe(amsterdamPosition?.[1]);
  expect(frankfurtPosition?.[2]).not.toBe(amsterdamPosition?.[2]);
  await locations.getByRole("list").getByRole("button", { name: /fra-web-01/ }).click();
  const inspector = targetInspector(page, "fra-web-01");
  await expect(inspector).toContainText("fra-web-01");
  await inspector.getByRole("button", { name: "Close", exact: true }).click();
  await expect(inspector).toBeHidden();
  await expect(frankfurt).toHaveAttribute("aria-pressed", "true");
  await expect(amsterdam).toHaveAttribute("aria-pressed", "false");
  await page.getByRole("textbox", { name: "Search targets, sites, roles, tags...", exact: true }).fill("fra-web-01");
  await expect(page.getByRole("button", { name: "1 targets · 1 sites", exact: true })).toBeVisible();
  await expect(amsterdam).toHaveCount(0);
  await page.getByRole("button", { name: "Fit all locations", exact: true }).click();
  await expect(frankfurt).toHaveAttribute("aria-pressed", "true");
  await expect(locations.getByRole("heading", { name: "Frankfurt DC", exact: true })).toBeVisible();
});

test("retained map refetches Sites after a real Inventory editor mutation", async ({ appPage: page }) => {
  await page.goto("/connections");
  await page.getByRole("button", { name: /^(Map|Карта)$/ }).click();
  await expect(page.getByRole("button", { name: /Unplaced targets \(2\)|Без координат \(2\)/ })).toBeVisible();
  await page.getByRole("button", { name: /^(Inventory|Инвентарь)$/ }).click();
  await page.getByRole("link", { name: /^(Resources|Ресурсы)$/ }).click();
  await page.getByRole("button", { name: /^(Actions|Действия): Remote Lab$/ }).click();
  await page.getByRole("menuitem", { name: /^(Edit|Редактировать)$/ }).click();
  await page.getByRole("spinbutton", { name: /^(Latitude|Широта)$/ }).fill("35.6762");
  await page.getByRole("spinbutton", { name: /^(Longitude|Долгота)$/ }).fill("139.6503");
  const saved = page.waitForResponse(response => response.url().includes("/inventory/sites/site-lab-") && response.request().method() === "PUT");
  await page.getByRole("button", { name: /^(Save Changes|Сохранить изменения)$/ }).click();
  expect((await saved).status()).toBe(200);
  await page.getByRole("link", { name: /^(Workspace|Рабочее пространство)$/ }).click();
  await expect(page.getByRole("button", { name: /Unplaced targets \(1\)|Без координат \(1\)/ })).toBeVisible();
  await expect(page.getByRole("button", { name: /10 targets · 3 sites|Объектов: 10 · площадок: 3/ })).toBeVisible();
});

test.describe("placed map targets on touch", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });
  for (const selection of ["marker", "list"]) {
    test(`${selection} selection stays behind the shared inspector`, async ({ appPage: page }) => {
      await page.goto("/connections");
      await page.getByRole("button", { name: /^(Map|Карта)$/ }).tap();
      if (selection === "marker") {
        await page.locator("svg").getByRole("button", { name: /Amsterdam Edge/ }).tap();
      } else {
        await page.getByRole("button", { name: /^(List|Список)$/ }).tap();
        await page.getByRole("button", { name: /9 targets · 2 sites|Объектов: 9 · площадок: 2/ }).tap();
        await page.getByRole("complementary", { name: /Targets by site|Объекты по площадкам/ }).getByRole("button", { name: /Amsterdam Edge/ }).tap();
      }
      await page.getByRole("list", { name: /Location targets|Объекты площадки/ }).getByRole("button", { name: /web-prod-01/ }).tap();
      const inspector = targetInspector(page, "web-prod-01");
      await expect(inspector).toBeVisible();
      await expectInspectorForeground(inspector);
      await inspector.getByRole("button", { name: /^(Info|Информация)$/ }).tap();
      await inspector.getByRole("button", { name: /^(Close|Закрыть)$/ }).tap();
      await expect(inspector).toBeHidden();
    });
  }
});

for (const mobile of [false, true]) {
  test.describe(`dense location ${mobile ? "touch" : "desktop"}`, () => {
    test.use({ viewport: mobile ? { width: 390, height: 844 } : { width: 1440, height: 1000 }, hasTouch: mobile });
    test("browses 500 targets with bounded rows and the real inspector", async ({ appPage: page }) => {
      await page.goto("/connections?lang=en&dense-map=1");
      const activate = async (locator: ReturnType<typeof page.getByRole>) => mobile ? locator.tap() : locator.click();
      await activate(page.getByRole("button", { name: "Map", exact: true }));
      const locations = page.getByRole("complementary", { name: "Targets by site" });
      const inspector = targetInspector(page, "ams-vm-499");
      const webInspector = targetInspector(page, "web-prod-01");
      await expect(page.getByRole("button", { name: "506 targets · 4 sites" })).toBeVisible();
      if (mobile) await activate(page.getByRole("button", { name: "List", exact: true }));
      if (process.env.MAP_QA_DIR) await page.screenshot({ path: `${process.env.MAP_QA_DIR}/${mobile ? "mobile" : "desktop"}-overview.png`, animations: "disabled" });
      await activate(locations.getByRole("button", { name: /Amsterdam Campus/ }));
      await expect(inspector).toHaveCount(0);
      await expect(locations.getByText("Targets: 501 · Sites: 2")).toBeVisible();
      await page.getByRole("combobox", { name: "Site in this location" }).selectOption({ label: "Amsterdam Edge (500)" });
      await expect(locations.getByText("500 of 501 targets")).toBeVisible();
      await expect(page.getByRole("button", { name: "Devices 250", exact: true })).toBeVisible();
      await expect(page.getByRole("button", { name: "VMs 250", exact: true })).toBeVisible();
      const rows = page.getByRole("list", { name: "Location targets" }).locator("li > button:first-child");
      await expect(rows).toHaveCount(25);
      const firstPage = await rows.allTextContents();
      const firstRow = await rows.first().innerText();
      if (process.env.MAP_QA_DIR) await page.screenshot({ path: `${process.env.MAP_QA_DIR}/${mobile ? "mobile" : "desktop"}-dense-list.png`, animations: "disabled" });
      await activate(page.getByRole("button", { name: "Next page", exact: true }));
      await expect(rows.first()).not.toHaveText(firstRow);
      const secondPage = await rows.allTextContents();
      expect(secondPage).toHaveLength(25);
      expect(secondPage.every(label => !firstPage.includes(label))).toBe(true);
      await activate(page.getByRole("button", { name: "VMs 250", exact: true }));
      await expect(locations.getByText("250 of 501 targets")).toBeVisible();
      const search = locations.getByRole("textbox", { name: "Search targets in this location" });
      await search.fill("AMS-VM-499");
      await expect(locations.getByText("1 of 501 targets")).toBeVisible();
      await expect(rows).toHaveCount(1);
      await expect(rows.first()).toContainText("ams-vm-499");
      if (mobile) await activate(rows.first());
      else { await rows.first().focus(); await rows.first().press("Enter"); }
      await expect(inspector).toContainText("ams-vm-499");
      await activate(inspector.getByRole("button", { name: "Info", exact: true }));
      if (mobile) await expectInspectorForeground(inspector);
      await activate(inspector.getByRole("button", { name: "Close", exact: true }));
      await expect(search).toHaveValue("AMS-VM-499");
      await search.fill("no-such-target");
      await expect(locations.getByText("No matching results")).toBeVisible();
      await activate(locations.getByRole("button", { name: "Clear search and filters", exact: true }).first());
      await expect(locations.getByRole("button", { name: "All 501" })).toBeVisible();
      await expect(rows).toHaveCount(25);
      await search.fill("web-prod-01");
      await activate(rows.first());
      await activate(webInspector.getByRole("button", { name: "Connections", exact: true }));
      await expect(webInspector.getByRole("button", { name: "SSH root", exact: true })).toBeVisible();
      if (process.env.MAP_QA_DIR) await page.screenshot({ path: `${process.env.MAP_QA_DIR}/${mobile ? "mobile" : "desktop"}-inspector.png`, animations: "disabled" });
      await activate(webInspector.getByRole("button", { name: "Close", exact: true }));
      await search.focus();
      await search.press("Escape");
      await expect(page.getByRole("button", { name: "506 targets · 4 sites" })).toBeFocused();
      await expect(search).toHaveCount(0);
      const unplaced = page.getByRole("button", { name: "Unplaced targets (2)" });
      await activate(unplaced);
      await expect(page.getByText("No site assigned", { exact: true })).toBeVisible();
      await expect(page.getByText("Site coordinates missing or invalid", { exact: true })).toBeVisible();
      await locations.getByRole("textbox", { name: "Search unplaced targets" }).press("Escape");
      await expect(page.getByRole("button", { name: "506 targets · 4 sites" })).toBeFocused();
      await expect(page.getByText("No site assigned", { exact: true })).toBeHidden();
      await activate(locations.getByRole("button", { name: /Amsterdam Campus/ }));
      await search.fill("web-prod-01");
      await activate(rows.first());
      const launched = page.waitForResponse(response => response.url().endsWith("/connections/sessions") && response.request().method() === "POST");
      await activate(webInspector.getByRole("button", { name: "SSH root", exact: true }));
      const response = await launched;
      expect(response.status()).toBe(200);
      expect((await response.json()).session).toMatchObject({ target_id: "00000000-0000-4000-8001-000000000001", protocol: "ssh" });
    });
  });
}
