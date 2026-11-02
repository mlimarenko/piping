import type { Locator } from "@playwright/test";
import { captureLayoutThemes, expect, test } from "./fixtures";

async function expectSeparated(label: Locator, value: Locator) {
  const left = (await label.boundingBox())!;
  const right = (await value.boundingBox())!;
  expect(left.x + left.width, "full label remains left of the value column").toBeLessThanOrEqual(right.x - 4);
  for (const element of [label, value]) {
    expect(await element.evaluate(node => node.scrollWidth <= node.clientWidth + 1), "text wraps within its assigned column").toBe(true);
  }
}

for (const width of [1440, 390]) {
  test(`inspector labels, values and copy controls remain separate at ${width}px`, async ({ page }) => {
    const longValue = "readable-value-".repeat(20);
    await page.addInitScript(value => {
      const wrap = (original: typeof window.fetch): typeof window.fetch => async (...args) => {
        const response = await original(...args);
        const input = args[0];
        const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url, location.href);
        if (!url.pathname.startsWith("/api/v1/inventory/devices/") || !response.ok) return response;
        const data = await response.clone().json();
        data.custom_fields = { long_readable_field_label_without_truncation: value };
        return new Response(JSON.stringify(data), { status: response.status, headers: response.headers });
      };
      let current = wrap(window.fetch);
      Object.defineProperty(window, "fetch", { configurable: true, get: () => current, set: value => { current = wrap(value); } });
    }, longValue);
    await page.setViewportSize({ width, height: 1000 });
    await page.goto("/connections");
    await page.getByRole("button", { name: "Catalogue view and actions", exact: true }).click();
    await page.getByRole("menuitem", { name: "Tree", exact: true }).click();
    await page.getByRole("button", { name: /^web-prod-01/ }).first().click();
    const inspector = page.getByRole("complementary", { name: "Selected target details", exact: true });
    const paneWidth = await inspector.locator("section").first().evaluate(node => node.clientWidth);
    if (width === 1440) expect(paneWidth).toBe(320);
    const details = inspector.getByRole("button", { name: "Details", exact: true });
    await details.click();
    const label = inspector.getByText("Device Type Name", { exact: true });
    const value = inspector.getByText("PowerEdge R650", { exact: true });
    await expect(value).toBeVisible();
    await expectSeparated(label, value);
    await test.info().attach("device-type-field-bounds", {
      body: JSON.stringify({ paneWidth, label: await label.boundingBox(), value: await value.boundingBox() }),
      contentType: "application/json",
    });
    const customFields = inspector.getByRole("button", { name: "Custom fields", exact: true });
    await customFields.scrollIntoViewIfNeeded();
    await customFields.click();
    const long = inspector.getByText(longValue, { exact: true });
    await long.scrollIntoViewIfNeeded();
    const row = long.locator("..");
    await expectSeparated(row.locator(":scope > div span").last(), long);
    const copy = row.getByRole("button");
    await expect(copy).toBeInViewport();
    const rowBox = (await row.boundingBox())!;
    const copyBox = (await copy.boundingBox())!;
    expect(copyBox.x + copyBox.width).toBeLessThanOrEqual(rowBox.x + rowBox.width + 1);
    await captureLayoutThemes(page, process.env.PWA117_PROOF_DIR, `inspector-field-bounds-${width}`);
  });
}
