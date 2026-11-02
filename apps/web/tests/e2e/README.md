# Playwright E2E Suite

Runs the real SPA against the **demo build**: `vite build --mode demo` produces
the same application bundle the public portal embeds, but every `/api/v1/*` call
is answered in-page by the MSW handlers in `src/demo/handlers.ts` over the
in-memory store in `src/demo/store.ts`. There is no backend, no database, and no
login step.

## What this suite gates — and what it does not

It gates the frontend: that every route in `src/app/App.tsx` still resolves and
renders without runtime errors, that the workspace boots, and that the CRUD and
navigation flows below behave. It does **not** gate backend behaviour — no
request in this lane reaches a server. Database and API contract are gated by
`coverage:backend` and the contract tests in `apps/api`.

Specs that needed a live backend (SSH provisioning, backup export/import, the
API sort/filter contract sweep, the inventory table-parity sweep) were removed
rather than pointed at mocks: asserting that a mock returns what the mock was
told to return is not a gate. Screenshot-only specs were removed for the same
reason — they asserted nothing.

## Running

```
cd apps/web
pnpm exec playwright install chromium --with-deps   # first time only
pnpm test:e2e
```

`playwright.config.ts` owns the whole lane: its `webServer` runs the demo build
and serves it with `scripts/serve-demo.mjs`. Nothing else needs to be up.

Two details that matter if you change the serving side:

- The build uses `--base=/`. The portal build emits under `/demo-app/`, where
  the suite's root-absolute `page.goto("/connections")` would not resolve.
- `dist-demo` contains `index.demo.html`, not `index.html`, so the static server
  serves that file as the SPA fallback. It answers `/api/*` with **404** on
  purpose: only MSW inside the page serves the API, and a 200 there would let a
  Node-side `page.request` call report `ok()` and pass on nothing.

HTML report lands in `tests/e2e/.html-report/`; traces and screenshots for
failures in `tests/e2e/.test-results/`.

## Suite layout

`tests/e2e/` holds 24 spec files — numbers `01`–`27`, with `17`, `21`, `22`,
`23` and `24` used twice and no file at `03` or `05`–`11` — plus the
shared `fixtures.ts` (`appPage` fixture, `expectRoutedPage` guard, console
collector, in-page API helper). All are `.spec.ts`:

- `01-pages-smoke`
- `02-inventory-crud`
- `04-connection-config-crud`
- `12-playbooks-ux`
- `13-page-header-layout`
- `14-nav-label-wrap`
- `15-branding-asset-actions`
- `16-connections-pane-layout`
- `17-inventory-ui-layout`, `17-terminal-edge-glyphs`
- `18-connections-ui-layout`
- `19-automation-observability-layout`
- `20-access-admin-account-layout`
- `21-connections-filter-footer`, `21-connections-workspace-sizing`
- `22-connections-inspector-scroll`, `22-unified-page-chrome`
- `23-api-docs-heading`, `23-connections-header-bounds`
- `24-connections-inspector-field-bounds`, `24-policy-editor-focus`
- `25-connections-catalogue-states`
- `26-ui-feedback`
- `27-connections-session-presentation`

## Conventions

- **No conditional assertions.** `if (await x.isVisible()) { expect(...) }` is a
  test that passes when the feature is missing. Assert unconditionally or delete
  the check.
- **A visible heading is not proof a route exists.** `NotFound` renders an
  `<h1>`. Use `expectRoutedPage()`, which also asserts the
  `data-testid="page-not-found"` marker is absent.
- Specs tolerate `en` and `ru` copy; the demo build defaults to `en`.
- Adding a route to `src/app/App.tsx` means adding it to `ROUTES` in
  `01-pages-smoke.spec.ts`.
- Demo data gaps are fixed in `src/demo/store.ts`, which is also what the public
  demo shows — so a fixture added for a test improves the demo too.
