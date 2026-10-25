import { mkdir } from "node:fs/promises";
import { expect, test as base, type Locator, type Page } from "@playwright/test";

const APP_READY_TIMEOUT_MS = 15_000;

export async function openActionsMenu(page: Page, trigger: Locator): Promise<Locator> {
  await trigger.click();
  const menu = page.getByRole("menu");
  await expect(menu).toBeVisible();
  return menu;
}

export async function captureLayoutThemes(page: Page, directory: string | undefined, name: string): Promise<void> {
  if (!directory) return;
  await mkdir(directory, { recursive: true });
  for (const theme of ["light", "dark"]) {
    await page.evaluate(value => {
      document.documentElement.classList.remove("light", "dark");
      document.documentElement.classList.add(value);
    }, theme);
    await page.screenshot({ path: `${directory}/${name}-${theme}.png`, fullPage: true, animations: "disabled" });
  }
}

interface AppFixtures {
  appPage: Page;
}

export async function waitForRoleSelector(
  page: Page,
  selector: `role=${string}`,
  timeout = APP_READY_TIMEOUT_MS,
): Promise<void> {
  await page.waitForSelector(selector, { state: "visible", timeout });
}

/**
 * Assert the current URL resolved to a real route rather than the catch-all.
 *
 * `NotFound` renders its own `<h1>`, so "a heading is visible" is satisfied by a
 * dead route — that is exactly how three removed routes stayed green in this
 * suite. `data-testid="page-not-found"` (see `src/pages/NotFound.tsx`) is the
 * language-neutral marker that distinguishes them.
 */
export async function expectRoutedPage(page: Page): Promise<void> {
  await waitForRoleSelector(page, "role=heading");
  await expect(page.getByRole("heading").first()).toBeVisible({
    timeout: APP_READY_TIMEOUT_MS,
  });
  await expect(
    page.getByTestId("page-not-found"),
    `${page.url()} rendered the NotFound catch-all — the route no longer exists`,
  ).toHaveCount(0);
}

/** Measure the rendered canvas, not class names that can agree while layouts drift. */
export async function expectManagementCanvas(page: Page): Promise<void> {
  const canvas = page.locator('#main-content [data-slot="page-layout"]');
  await expect(canvas).toHaveCount(1);
  const geometry = await canvas.evaluate(element => {
    const main = document.getElementById("main-content")!;
    const style = getComputedStyle(element);
    return {
      width: element.getBoundingClientRect().width,
      available: main.clientWidth,
      left: Number.parseFloat(style.paddingLeft),
      right: Number.parseFloat(style.paddingRight),
      headerPadding: Number.parseFloat(getComputedStyle(document.querySelector('[data-slot="app-header"]')!).paddingLeft),
      viewport: innerWidth,
      document: [document.documentElement.clientWidth, document.documentElement.scrollWidth],
    };
  });
  expect(Math.abs(geometry.width - geometry.available)).toBeLessThanOrEqual(1);
  expect(geometry.left).toBe(geometry.viewport >= 768 ? 24 : 16);
  expect(geometry.right).toBe(geometry.left);
  expect(geometry.headerPadding).toBe(geometry.left);
  expect(geometry.document[1]!).toBeLessThanOrEqual(geometry.document[0]! + 1);
  const tabs = page.locator('.section-tab-list .section-tab:visible');
  for (const tab of await tabs.all()) {
    expect((await tab.boundingBox())?.height).toBe(40);
  }
  for (const search of await page.locator('[data-slot="table-toolbar"] [data-slot="search-input"]:visible').all()) {
    const size = await search.evaluate(element => ({ width: element.getBoundingClientRect().width, available: element.parentElement!.clientWidth }));
    expect(Math.abs(size.width - Math.min(geometry.viewport >= 640 ? 288 : size.available, size.available))).toBeLessThanOrEqual(1);
  }
  if (geometry.viewport < 640) {
    for (const heading of await page.locator('[data-slot="surface-card-heading"]:visible').all()) {
      const widths = await heading.evaluate(element => {
        const parent = element.parentElement!;
        const style = getComputedStyle(parent);
        return [element.getBoundingClientRect().width, parent.clientWidth - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight)];
      });
      expect(Math.abs(widths[0]! - widths[1]!)).toBeLessThanOrEqual(1);
    }
  }
}

export async function waitForConnectionsPageReady(page: Page): Promise<void> {
  // The connections search textbox is the stable
  // role-level signal that the workspace has rendered.
  await waitForRoleSelector(page, "role=textbox");
  await expect(page.getByPlaceholder(/search|поиск/i).first()).toBeVisible({
    timeout: APP_READY_TIMEOUT_MS,
  });
  await expect(page.getByTestId("page-not-found")).toHaveCount(0);
}

/** Subset of Playwright's APIResponse that the specs actually use. */
export interface ApiResult {
  ok(): boolean;
  status(): number;
  text(): Promise<string>;
  json(): Promise<unknown>;
}

/**
 * Issue an API call FROM INSIDE THE PAGE.
 *
 * Playwright's `page.request` is a Node-side client: it shares the cookie jar
 * but never goes through the browser, so the MSW service worker that serves
 * this lane's API never sees it. Specs that seeded or verified state through
 * `page.request` therefore talked to the static file server instead of the
 * mock backend the UI was reading — which is why `scripts/serve-demo.mjs`
 * answers `/api/*` with 404 rather than the SPA fallback: a 200 there would
 * have made `response.ok()` true and turned those calls into silent passes.
 *
 * Routing them through `page.evaluate` puts them behind the same service
 * worker as the UI, so a spec seeds and asserts against exactly the state the
 * screen under test is rendering.
 */
export async function apiRequest(
  page: Page,
  method: "GET" | "POST" | "PUT" | "PATCH" | "DELETE",
  path: string,
  opts: { data?: unknown; params?: Record<string, string | number | boolean> } = {},
): Promise<ApiResult> {
  // The mock backend is the page: `src/demo/store.ts` is module state inside the
  // bundle, and the MSW service worker only exists once the app has booted.
  // Calling this before the first navigation would hit `about:blank`, where
  // there is no origin to resolve against and nothing serving /api.
  expect(
    page.url(),
    "apiRequest() needs the app loaded first — the demo API lives inside the page",
  ).toMatch(/^https?:/);

  const result = await page.evaluate(
    async ([method, path, rawParams, rawData]) => {
      const url = new URL(path, window.location.origin);
      for (const [key, value] of Object.entries(rawParams ?? {})) {
        url.searchParams.set(key, String(value));
      }
      const response = await fetch(url.toString(), {
        method,
        headers: rawData === undefined ? undefined : { "content-type": "application/json" },
        body: rawData === undefined ? undefined : JSON.stringify(rawData),
      });
      return { status: response.status, ok: response.ok, body: await response.text() };
    },
    [method, path, opts.params ?? null, opts.data] as const,
  );

  return {
    ok: () => result.ok,
    status: () => result.status,
    text: () => Promise.resolve(result.body),
    json: () => Promise.resolve(JSON.parse(result.body) as unknown),
  };
}

/**
 * Collect runtime console errors on a page. Returns an accessor that yields
 * the accumulated list at assertion time.
 *
 * React logs many recoverable dev warnings via `console.error`; filter those
 * out at the call site if needed. The smoke suite treats any unfiltered
 * `error`-level console line as a failure, except for Chromium's transient
 * `net::ERR_NETWORK_CHANGED` resource noise, which is browser-level transport
 * churn rather than an application runtime error.
 */
export function collectConsoleErrors(page: Page): () => string[] {
  const errors: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      const text = msg.text();
      if (text.includes("Failed to load resource: net::ERR_NETWORK_CHANGED")) {
        return;
      }
      errors.push(text);
    }
  });
  page.on("pageerror", (err) => {
    errors.push(`pageerror: ${err.message}`);
  });
  return () => errors.slice();
}

export { expect };

/**
 * The suite runs against the demo build (`vite build --mode demo`), which boots
 * the real SPA against the MSW handlers in `src/demo/handlers.ts`. There is no
 * backend and no login step: `GET /api/v1/auth/me` is answered with a
 * full-capability admin, so `RequireCapability` admits every route.
 *
 * `appPage` therefore only guarantees "the workspace has booted", which is the
 * precondition every spec below actually needed from the old login fixture.
 */
export const test = base.extend<AppFixtures>({
  appPage: async ({ page }, use) => {
    await page.goto("/connections");
    await waitForConnectionsPageReady(page);
    await use(page);
  },
});
