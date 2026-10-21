import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright configuration for the piping frontend E2E suite.
 *
 * Target: the DEMO build — the real SPA served as static files, with every
 * `/api/v1/*` call answered by the MSW handlers in `src/demo/handlers.ts`.
 * No backend, no database, no seeded credentials.
 *
 * Why not the docker compose stack this used to target: that lane needed a full
 * `docker compose up` plus an `admin/piping123` seed that no longer authenticates,
 * so nothing could run it — including CI, which never referenced it at all. What
 * this suite actually gates is frontend routing, rendering, and consumption of
 * the API contract; the demo build exercises all three and costs one `vite build`.
 * Backend behaviour is gated by `coverage:backend`, not from here.
 *
 * `--base=/` is required: the demo build normally emits under `/demo-app/` for
 * the portal embed, and root-absolute `page.goto("/connections")` would not
 * resolve inside that base.
 */
const PORT = Number(process.env.PIPING_DEMO_PORT ?? 4173);

export default defineConfig({
  testDir: "./tests/e2e",
  outputDir: "./tests/e2e/.test-results",

  // The MSW store (`src/demo/store.ts`) lives inside the page, so specs are
  // already isolated from each other — but the runner this lane targets is
  // memory-bound (it has OOM-killed rustc twice), and one chromium is the
  // cheap, safe default.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: !!process.env.CI,

  reporter: process.env.CI
    ? [["list"], ["junit", { outputFile: "./tests/e2e/.test-results/junit.xml" }]]
    : [["list"], ["html", { outputFolder: "./tests/e2e/.html-report", open: "never" }]],

  webServer: {
    // Invoked through the local vite binary rather than a pnpm script so the
    // lane owns its build end-to-end and never inherits a package-manager
    // preflight (pnpm's dep verification) as a way to fail.
    command: "node ./node_modules/vite/bin/vite.js build --mode demo --base=/ && node scripts/serve-demo.mjs",
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: "pipe",
    stderr: "pipe",
  },

  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "off",
  },

  projects: [
    {
      name: "desktop-chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
