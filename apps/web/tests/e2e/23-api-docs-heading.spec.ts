import { expect, expectRoutedPage, test } from "./fixtures";

const spec = {
  openapi: "3.1.0",
  info: {
    title: "Piping API",
    version: "1.2.3",
    summary: "Control plane API",
    description: "Manage **inventory** and connections.",
    termsOfService: "https://example.com/terms",
    contact: { name: "API support", url: "https://example.com/support", email: "support@example.com" },
    license: { name: "API license", url: "https://example.com/license" },
  },
  externalDocs: { description: "API guide", url: "https://example.com/guide" },
  paths: {},
};

for (const viewport of [
  { name: "desktop light", width: 1440, height: 900, theme: "light" },
  { name: "mobile dark", width: 390, height: 844, theme: "dark" },
]) {
  test(`API documentation preserves metadata below the sole application heading: ${viewport.name}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    // MSW owns demo API requests, so page.route cannot replace its responses.
    // Substitute only the spec-fetch boundary, before the real SPA/SDK boots.
    await page.addInitScript(document => {
      const originalFetch = window.fetch.bind(window);
      window.fetch = (input, init) => {
        const url = new URL(input instanceof Request ? input.url : String(input), window.location.href);
        if (url.origin === window.location.origin && url.pathname === "/api/v1/openapi/piping.openapi.yaml") {
          return Promise.resolve(new Response(JSON.stringify(document), {
            headers: { "Content-Type": "application/json" },
          }));
        }
        return originalFetch(input, init);
      };
    }, spec);
    await page.goto("/api-docs");
    await expectRoutedPage(page);
    await page.evaluate(theme => {
      document.documentElement.classList.remove("light", "dark");
      document.documentElement.classList.add(theme);
    }, viewport.theme);

    const infoHeading = page.getByRole("heading", { level: 2, name: /Piping API/ });
    await expect(infoHeading).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
    await expect(page.locator('[data-slot="app-header"]').getByRole("heading", { level: 1 }))
      .toHaveAttribute("data-slot", "app-header-title");
    await expect(page.getByRole("main").getByRole("heading", { level: 1 })).toHaveCount(0);
    await expect(infoHeading).toContainText("1.2.3");
    await expect(infoHeading).toContainText(/OAS\s*3\.1/);
    await expect(page.getByText("Control plane API", { exact: true })).toBeVisible();
    await expect(page.getByText("inventory", { exact: true })).toBeVisible();
    await expect(page.getByRole("link", { name: /piping.openapi.yaml/ }))
      .toHaveAttribute("href", /\/api\/v1\/openapi\/piping.openapi.yaml$/);
    for (const [name, href] of [
      ["Terms of service", "https://example.com/terms"],
      ["API support - Website", "https://example.com/support"],
      ["Send email to API support", "mailto:support@example.com"],
      ["API license", "https://example.com/license"],
      ["API guide", "https://example.com/guide"],
    ] as const) {
      const link = page.getByRole("link", { name, exact: true });
      await expect(link).toHaveAttribute("href", href);
      await expect(link).toBeVisible();
    }
  });
}
