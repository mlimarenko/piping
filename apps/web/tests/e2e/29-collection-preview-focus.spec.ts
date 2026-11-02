import type { Locator, Page } from "@playwright/test";
import { apiRequest, expect, test } from "./fixtures";

async function observeFocus(page: Page, target: Locator, name: string) {
  await expect(target).toBeFocused();
  await expect(target).toBeVisible();
  await target.evaluate(async element => { await Promise.all(element.getAnimations().map(animation => animation.finished.catch(() => {}))); });
  const observation = await target.evaluate(element => ({
    tag: element.tagName, label: element.getAttribute("aria-label"),
    visible: element.getClientRects().length > 0, focusVisible: element.matches(":focus-visible"),
    shadow: getComputedStyle(element).boxShadow,
  }));
  expect(observation.focusVisible).toBe(true);
  expect(observation.shadow).toContain("2px");
  await test.info().attach(name, { body: JSON.stringify(observation), contentType: "application/json" });
  const directory = process.env.PIPING_FOCUS_ARTIFACTS;
  if (directory) await page.screenshot({ path: `${directory}/${name}.png`, animations: "disabled" });
}

async function setSearchBehindPreview(search: Locator, value: string) {
  await search.evaluate((input, next) => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, next);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }, value);
}

for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
  for (const theme of ["light", "dark"]) {
    const variant = `${viewport.width}-${theme}`;
    test.describe(variant, () => {
      test.use({ viewport });
      test("audit preview survives and then loses its opener through a query", async ({ appPage: page }) => {
        await page.goto("/audit-logs?lang=en");
        await page.getByRole("button", { name: "Details", exact: true }).first().waitFor();
        await page.evaluate(value => { document.documentElement.classList.remove("light", "dark"); document.documentElement.classList.add(value); }, theme);
        const opener = page.getByRole("button", { name: "Details", exact: true }).first();
        await opener.focus();
        await opener.press("Enter");
        const dialog = page.getByRole("dialog");
        await expect(dialog).toBeVisible();
        await dialog.press("Escape");
        await observeFocus(page, opener, `audit-opener-${variant}`);
        await opener.press("Enter");
        await expect(dialog).toBeVisible();
        const response = await apiRequest(page, "GET", "/api/v1/audit/events");
        const data = await response.json() as { items: { id: string }[] };
        expect(data.items.length).toBeGreaterThan(1);
        const entry = data.items[0];
        if (!entry) throw new Error("Demo audit fixture is missing");
        const deleted = await apiRequest(page, "DELETE", `/api/v1/audit/events/${entry.id}`);
        expect(deleted.ok()).toBe(true);
        const request = page.waitForResponse(response => response.url().includes("/audit/events?q=PWA161"));
        const search = page.getByPlaceholder("Search event, target, summary...");
        await setSearchBehindPreview(search, "PWA161");
        const refetched = await request;
        expect(refetched.status()).toBe(200);
        await test.info().attach(`audit-query-${variant}`, { body: JSON.stringify({ url: refetched.url(), status: refetched.status(), body: await refetched.json() }), contentType: "application/json" });
        await expect(page.locator("tr button[aria-label=Details]")).toHaveCount(data.items.length - 1);
        await expect(dialog).toBeVisible();
        await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
        await observeFocus(page, search, `audit-anchor-${variant}`);
        await page.keyboard.press("Tab");
        await expect(page.getByRole("combobox", { name: "Channel", exact: true })).toBeFocused();
      });

      test("environment preview keeps its row or stable region after a query", async ({ appPage: page }) => {
        await page.goto("/automation/execution-environments?lang=en");
        const row = page.locator("tr", { hasText: "Default Ansible" });
        await expect(row).toBeVisible();
        await page.evaluate(value => { document.documentElement.classList.remove("light", "dark"); document.documentElement.classList.add(value); }, theme);
        await row.focus();
        await row.press(" ");
        const dialog = page.getByRole("dialog", { name: "Default Ansible" });
        await expect(dialog).toBeVisible();
        await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
        await expect(row).toBeFocused();
        await expect(row).toBeVisible();
        await test.info().attach(`environment-pointer-close-${variant}`, { body: JSON.stringify(await row.evaluate(element => ({ focused: document.activeElement === element, focusVisible: element.matches(":focus-visible") }))), contentType: "application/json" });
        await row.press("Enter");
        await expect(dialog).toBeVisible();
        const close = dialog.getByRole("button", { name: "Close", exact: true }).last();
        await page.keyboard.press("Tab");
        for (let step = 0; step < 8 && !await close.evaluate(element => element === document.activeElement); step++) await page.keyboard.press("Tab");
        await expect(close).toBeFocused();
        await page.keyboard.press("Enter");
        await observeFocus(page, row, `environment-keyboard-close-${variant}`);
        await row.press("Enter");
        await expect(dialog).toBeVisible();
        await dialog.press("Escape");
        await observeFocus(page, row, `environment-opener-${variant}`);
        await page.keyboard.press("Tab");
        const actions = row.getByRole("button", { name: "Actions: Default Ansible" });
        await expect(actions).toBeFocused();
        await actions.press("Enter");
        await page.getByRole("menuitem", { name: "Details", exact: true }).click();
        await expect(dialog).toBeVisible();
        const response = await apiRequest(page, "GET", "/api/v1/playbooks/execution-environments");
        const data = await response.json() as { items: { id: string }[] };
        for (const environment of data.items) expect((await apiRequest(page, "DELETE", `/api/v1/playbooks/execution-environments/${environment.id}`)).ok()).toBe(true);
        const request = page.waitForResponse(response => response.url().includes("/playbooks/execution-environments") && response.request().method() === "GET");
        await page.getByRole("button", { name: "Refresh", includeHidden: true }).evaluate(button => {
          if (!(button instanceof HTMLButtonElement)) throw new Error("Refresh must be a button");
          button.click();
        });
        const refetched = await request;
        await test.info().attach(`environment-query-${variant}`, { body: JSON.stringify({ url: refetched.url(), status: refetched.status(), body: await refetched.json() }), contentType: "application/json" });
        await expect(row).toHaveCount(0);
        await expect(dialog).toBeVisible();
        await dialog.press("Escape");
        await observeFocus(page, page.getByRole("region", { name: "Execution environments", exact: true }), `environment-anchor-${variant}`);
        await page.keyboard.press("Tab");
        await expect(page.locator(":focus")).not.toHaveText(/Skip to content/);
      });
    });
  }
}

test("dataflow Sheet preserves list/topology and restores search after a real update removes the opener", async ({ appPage: page }) => {
  await page.addInitScript(() => {
    const flow = {
      id: "pwa161-flow", name: "PWA161 directed flow", status: "active", transport: "tcp", destination_ports: [443], allow_cross_vrf: false,
      source: { object_type: "all", address_mode: "default", ipaddresses: [] },
      destination: { object_type: "internet", address_mode: "default", ipaddresses: [] },
    };
    const nativeFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const request = new Request(input, init);
      const path = new URL(request.url).pathname;
      if (path === "/api/v1/inventory/dataflows/endpoint-resolution") return Response.json({ endpoint: await request.json(), members: [], addresses: [], ipaddresses: [], default_ipaddresses: [], diagnostics: [] });
      if (path === "/api/v1/inventory/dataflows") return Response.json({ items: [flow], total: 1, limit: 100, offset: 0 });
      if (path === `/api/v1/inventory/dataflows/${flow.id}` && request.method === "PUT") { Object.assign(flow, await request.json()); return Response.json(flow); }
      return nativeFetch(input, init);
    };
  });
  await page.goto("/inventory/dataflows?lang=en&status=active");
  const opener = page.getByRole("button", { name: "Inspect PWA161 directed flow" });
  await opener.focus();
  await opener.press("Enter");
  const dialog = page.getByRole("dialog", { name: "PWA161 directed flow" });
  await expect(dialog).toBeVisible();
  await dialog.press("Escape");
  await observeFocus(page, opener, "dataflow-list-opener");
  await page.getByRole("tab", { name: "Topology", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Topology", exact: true })).toHaveAttribute("data-state", "active");
  await expect(page).toHaveURL(/view=topology/);
  const topologyOpener = page.getByRole("button", { name: /Inspect PWA161 directed flow/ });
  await topologyOpener.focus();
  await topologyOpener.press("Enter");
  await expect(dialog).toBeVisible();
  await dialog.press("Escape");
  await observeFocus(page, topologyOpener, "dataflow-topology-opener");
  await topologyOpener.press("Enter");
  await dialog.getByRole("button", { name: "Actions for PWA161 directed flow" }).click();
  await page.getByRole("menuitem", { name: "Disable", exact: true }).click();
  await expect(topologyOpener).toHaveCount(0);
  await expect(dialog).toBeVisible();
  await dialog.press("Escape");
  const search = page.getByPlaceholder("Search names or endpoints…");
  await observeFocus(page, search, "dataflow-search-anchor");
  await page.keyboard.press("Tab");
  await expect(page.getByRole("combobox", { name: "Status", exact: true })).toBeFocused();
  await expect(page.getByRole("tab", { name: "Topology", exact: true })).toHaveAttribute("data-state", "active");
});
