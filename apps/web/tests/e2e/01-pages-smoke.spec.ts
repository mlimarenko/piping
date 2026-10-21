import type { Page } from "@playwright/test";
import {
  collectConsoleErrors,
  expect,
  test,
  waitForConnectionsPageReady,
  expectRoutedPage,
} from "./fixtures";

/**
 * Page-by-page smoke test.
 *
 * Route list mirrors the static, parameterless routes in `src/app/App.tsx`.
 * Every omission, with its reason:
 *   - `/auth/*`, `/connections/detached*` — outside the authenticated shell.
 *   - anything with a `:param` — needs a fixture id, not a smoke pass.
 *
 * Each route below is asserted with `expectRoutedPage`, which requires a visible
 * heading AND the absence of the `data-testid="page-not-found"` catch-all marker
 * (see `src/pages/NotFound.tsx`); a URL that falls through to the catch-all fails
 * here.
 */
const ROUTES: readonly string[] = [
  "/connections",
  "/connections/config",
  "/connections/config/new",
  "/connections/active",
  "/connections/proxies",
  "/connections/proxies/new",
  "/connections/proxies/assignments/new",
  "/inventory/resources",
  "/inventory/dataflows",
  "/inventory/dataflows/new",
  "/inventory/custom-fields",
  "/inventory/templates",
  "/inventory/templates/new",
  "/inventory/sync",
  "/inventory/sync/mappings",
  "/inventory/sync/mappings/new",
  "/playbooks",
  "/automation/projects",
  "/automation/job-templates",
  "/automation/execution-environments",
  "/automation/assignments",
  "/automation/target-sets",
  "/automation/schedules",
  "/automation/service-principals",
  "/automation/runs",
  "/automation/secrets",
  "/observability",
  "/observability/sources/new",
  "/observability/metrics/new",
  "/observability/sets/new",
  "/observability/alert-sources/new",
  "/api-docs",
  "/mcp",
  "/access-control",
  "/audit-logs",
  "/administration/license",
  "/administration/webhooks",
  "/administration/branding",
  "/administration/settings",
  "/administration/backup",
  "/user/settings",
  "/user/settings/credentials/new",
  "/user/settings/tokens/new",
];

/**
 * React/React-Router occasionally emit recoverable `console.error` lines in
 * dev/prod bundles (e.g. ResizeObserver loop limit, DevTools hint, failed
 * optional HEAD requests). The smoke test should flag real runtime crashes,
 * not these. Extend this list only when you've confirmed the line is benign.
 */
const IGNORED_CONSOLE_PATTERNS: readonly RegExp[] = [
  /ResizeObserver loop/i,
  /Download the React DevTools/i,
];

async function waitForRouteReady(page: Page, route: string): Promise<void> {
  if (route === "/connections") {
    await waitForConnectionsPageReady(page);
    return;
  }
  await expectRoutedPage(page);
}

test.describe("page smoke", () => {
  for (const route of ROUTES) {
    test(`renders ${route} without runtime errors`, async ({ appPage: page }) => {
      const getErrors = collectConsoleErrors(page);

      await page.goto(route);
      await page.waitForLoadState("networkidle");
      await waitForRouteReady(page, route);

      // No screenshot here on purpose: nothing reads it, and writing one per
      // route uploaded 20-odd full-page PNGs from every green CI run. Playwright
      // already captures a screenshot and a trace when a test fails.
      const errors = getErrors().filter(
        (line) => !IGNORED_CONSOLE_PATTERNS.some((rx) => rx.test(line)),
      );
      expect(
        errors,
        `console errors on ${route}:\n${errors.join("\n")}`,
      ).toEqual([]);
    });
  }
});
