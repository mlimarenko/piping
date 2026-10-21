import type { Page } from "@playwright/test";

import { expect, test, expectRoutedPage } from "./fixtures";

test("empty table content stays inside the narrow scroll viewport", async ({ appPage: page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/inventory/sync/mappings");
  const viewport = page.locator("[data-table-scroll-viewport]");
  const content = viewport.locator("td > div").first();
  await expect(content.getByRole("button")).toBeVisible();
  const available = await viewport.boundingBox();
  const notice = await content.boundingBox();
  expect(available).not.toBeNull();
  expect(notice).not.toBeNull();
  expect(notice!.width).toBeLessThanOrEqual(available!.width);
  expect(notice!.x).toBeGreaterThanOrEqual(available!.x);
  expect(notice!.x + notice!.width).toBeLessThanOrEqual(available!.x + available!.width);
});

/**
 * Global page chrome retains the responsive geometry checks from PWA-66.
 *
 * The pre-fix header was a single non-wrapping row: a `truncate` title, a
 * `shrink-0` badge that could not move off the title line, and a `shrink-0`
 * action row. On a narrow viewport the title clipped to an ellipsis and the
 * badge and action row ran past the header's right edge — inside a clipping
 * ancestor, so the page itself did not always scroll. That is why the
 * assertions below are header-specific. The title and actions now live in
 * the shell's TopBar, but the same wrapping invariants remain:
 *
 *   1. root       — a natural-height wrapping row;
 *   2. title row  — `flex-wrap`, so the badge can drop to its own line;
 *   3. title      — `min-w-0 break-words` instead of `truncate`;
 *   4. actions    — a wrapping, shrinkable row (`min-w-0 max-w-full flex-wrap`
 *                   plus `[&>*]` wrapping for a nested action group).
 *
 * The "fits" cases guard ordinary content. The "stress" cases load the real
 * pages, overwrite the real `<h1>`/badge text with an unbroken string, clone
 * the real action buttons until they must wrap, then assert measured geometry
 * (line counts, line positions, client/scroll widths). Each stress case also
 * applies the pre-fix class for one hunk in the live page — never in the shared
 * source — and asserts the same geometry predicate goes false, which is what
 * makes each hunk load-bearing rather than incidentally present.
 *
 * Every stress case waits for the real badge element: on
 * `/inventory/custom-fields` it renders only after the definitions query
 * resolves, so measuring before it appears would race.
 */

// The Russian inventory title reproduces the mobile clipping reported in PWA-66.
test.use({ locale: "ru-RU" });

const ROUTES = ["/inventory/resources", "/inventory/custom-fields"] as const;

const VIEWPORTS = [
  { label: "mobile", width: 390, height: 844 },
  { label: "desktop", width: 1440, height: 900 },
] as const;

test.describe("shared editor foundation", () => {
  for (const viewport of VIEWPORTS) {
    for (const scale of [1, 1.25]) {
      test(`short editor stays bounded with adjacent actions at ${viewport.label}, scale ${scale}`, async ({ appPage: page }) => {
        await page.setViewportSize(viewport);
        await page.goto("/inventory/resources/Manufacturers/new");
        await expectRoutedPage(page);
        const editor = page.getByTestId("entity-editor-page");
        const name = editor.getByPlaceholder(/enter name|введите имя/i);
        await expect(name).toBeVisible();
        // Exercise rem sizing on the rendered application, not generated CSS text.
        // Scale normalization and application are covered by ui-scale.test.ts.
        await page.evaluate(scale => { document.documentElement.style.fontSize = `${16 * scale}px`; }, scale);
        const sizes = await editor.evaluate(element => {
          const input = element.querySelector("input")!;
           const heading = document.querySelector('[data-slot="app-header"] h1')!;
          const label = element.querySelector("label")!;
          const footer = element.querySelector("footer")!;
          const action = footer.querySelector("button")!;
          const fields = element.querySelector('[data-slot="form-section-content"]')!;
          const main = element.closest("main")!;
          return {
            body: Number.parseFloat(getComputedStyle(document.body).fontSize),
            heading: Number.parseFloat(getComputedStyle(heading).fontSize),
            input: Number.parseFloat(getComputedStyle(input).fontSize),
            label: Number.parseFloat(getComputedStyle(label).fontSize),
            actionHeight: action.getBoundingClientRect().height,
            footerGap: footer.getBoundingClientRect().top - fields.getBoundingClientRect().bottom,
            editorWidth: element.getBoundingClientRect().width,
            editorHeight: element.getBoundingClientRect().height,
            mainHeight: main.getBoundingClientRect().height,
            scrollWidth: main.scrollWidth,
            clientWidth: main.clientWidth,
          };
        });
        expect(sizes.body).toBeCloseTo(15 * scale);
         expect(sizes.heading).toBeCloseTo(16 * scale);
        expect(sizes.input).toBeCloseTo((viewport.width < 768 ? 16 : 15) * scale);
        expect(sizes.label).toBeCloseTo(15 * scale);
        expect(sizes.actionHeight).toBeCloseTo(36 * scale);
        expect(sizes.footerGap).toBeGreaterThanOrEqual(0);
        expect(sizes.footerGap).toBeLessThanOrEqual(32 * scale);
        expect(sizes.editorWidth).toBeLessThanOrEqual(960);
        expect(sizes.scrollWidth).toBeLessThanOrEqual(sizes.clientWidth + 1);
        if (viewport.label === "desktop") expect(sizes.editorHeight).toBeLessThan(sizes.mainHeight);
        await name.fill("Foundation check");
        await editor.getByRole("button", { name: /^(cancel|отмена)$/i }).click();
        await expect(page.getByRole("alertdialog")).toBeVisible();
        await page.getByRole("button", { name: /continue editing|продолжить редактирование/i }).click();
        await expect(name).toHaveValue("Foundation check");
      });
    }
  }
});

test("mobile navigation contains its sidebar and restores toggle focus", async ({ appPage: page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/inventory/resources");
  await expectRoutedPage(page);
  const toggle = page.locator("#mobile-navigation-toggle");
  for (const scale of [1, 1.25]) {
    await page.evaluate(value => { document.documentElement.style.fontSize = `${16 * value}px`; }, scale);
    await toggle.click();
    const drawer = page.getByRole("dialog");
    await expect(drawer).toBeVisible();
    await expect.poll(() => drawer.evaluate(element => {
      const sidebar = element.querySelector("aside")!;
      return sidebar.getBoundingClientRect().right - element.getBoundingClientRect().right;
    })).toBeLessThanOrEqual(1);
    const administration = drawer.getByRole("button", { name: /administration|администрирование/i, exact: true });
    await administration.click();
    await expect(administration).toHaveAttribute("aria-expanded", "true");
    await page.keyboard.press("Escape");
    await expect(drawer).toBeHidden();
    await expect(toggle).toBeFocused();
  }
});

test.describe("long embedded editor viewport", () => {
  for (const viewport of [
    { label: "mobile touch", width: 390, height: 844, touch: true },
    { label: "desktop wheel", width: 1440, height: 600, touch: false },
  ]) {
    test.describe(viewport.label, () => {
    test.use({ hasTouch: viewport.touch });
    test(`custom-field editor scrolls to its final field and footer at ${viewport.label}`, async ({ appPage: page }) => {
      await page.setViewportSize(viewport);
      await page.goto("/inventory/custom-fields");
      await expectRoutedPage(page);
       const invoker = page.locator('[data-slot="app-header"] [data-slot="page-actions"] button').filter({ hasText: /new field|новое поле/i });
      await invoker.click();
      const host = page.locator("[data-entity-editor-host]");
       await expect(host.getByRole("heading", { level: 2, name: /create custom field|создать пользовательское поле/i })).toBeVisible();
      await expect.poll(() => invoker.evaluate(element => element.closest("[inert]") !== null)).toBe(true);
      // An inert workspace cannot take focus even when the invoker stays mounted.
      expect(await invoker.evaluate(element => {
        (element as HTMLElement).focus();
        return document.activeElement === element;
      })).toBe(false);
      const viewportBox = await host.boundingBox();
      expect(viewportBox).not.toBeNull();
      expect(await host.evaluate(element => element.scrollHeight > element.clientHeight)).toBe(true);
      const before = await host.evaluate(element => element.scrollTop);
      const x = viewportBox!.x + viewportBox!.width / 2;
      const y = viewportBox!.y + Math.min(200, viewportBox!.height / 2);
      if (viewport.touch) {
        const cdp = await page.context().newCDPSession(page);
        const touchX = viewportBox!.x + 8;
        const startY = viewportBox!.y + viewportBox!.height - 60;
        const endY = viewportBox!.y + 60;
        await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: touchX, y: startY }] });
        for (let step = 1; step <= 10; step++) {
          await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: touchX, y: startY + (endY - startY) * step / 10 }] });
          await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
        }
        await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
        await cdp.detach();
      } else {
        await page.mouse.move(x, y);
        await page.mouse.wheel(0, 2000);
      }
      await expect.poll(() => host.evaluate(element => element.scrollTop), {
        message: "user scrolling must advance the embedded editor's own viewport",
      }).toBeGreaterThan(before);
      const scope = host.getByRole("combobox", { name: /applies to|применяется к/i });
      await expect(scope).toBeEnabled();
      await scope.scrollIntoViewIfNeeded();
      await expect(scope).toBeInViewport({ ratio: 1 });
      await scope.click();
      await expect(scope).toHaveAttribute("aria-expanded", "true");
      await page.keyboard.press("Escape");
      const cancel = host.locator("footer").getByRole("button", { name: /^(cancel|отмена)$/i });
      await cancel.scrollIntoViewIfNeeded();
      await expect(cancel).toBeInViewport({ ratio: 1 });
      for (const action of await host.locator("footer").getByRole("button").all()) {
        expect((await action.boundingBox())?.height, "primary editor actions use the standard control height").toBe(36);
      }
      await expect(host.locator("footer").getByRole("button", { name: /^(create|создать)$/i })).toBeInViewport({ ratio: 1 });
      expect(await invoker.evaluate(element => element.closest("[inert]") !== null)).toBe(true);
      await cancel.click();
      await expect(host).toHaveCount(0);
      await expect(invoker).toBeFocused();
      expect(await invoker.evaluate(element => element.closest("[inert]") !== null)).toBe(false);
    });
    });
  }
});

/** One unbroken token: only `overflow-wrap` can break it, so a `truncate` title clips it. */
const LONG_TITLE = "W".repeat(180);
/** Wide but space-separated, so it is a badge that wraps rather than one that overflows. */
const LONG_BADGE = "Бейдж с очень длинным значением для проверки";
/** Enough extra controls that the action row cannot fit on one line at either width. */
const ACTION_CLONES = 24;

/** The three hunks whose necessity the stress cases prove. */
type Hunk = "title" | "titleRow" | "actions";

interface Rect {
  left: number;
  right: number;
  top: number;
  bottom: number;
  width: number;
  height: number;
}

interface HeaderGeometry {
  header: Rect;
  headerClientWidth: number;
  headerScrollWidth: number;
  title: Rect;
  titleClientWidth: number;
  titleScrollWidth: number;
  titleClientHeight: number;
  titleScrollHeight: number;
  titleLineCount: number;
  titleLastLineBottom: number;
  badge: Rect | null;
  actionRects: Rect[];
  actionRowCount: number;
  actionsWithinHeader: boolean;
  originalControlsUsable: boolean;
  pageClientWidth: number;
  pageScrollWidth: number;
}

interface ClassSnapshot {
  title: string;
  titleRow: string;
  actions: string;
}

/**
 * The badge is data-dependent on `/inventory/custom-fields` (it renders only
 * once the definitions query resolves), and the action button likewise follows
 * capabilities. Wait for the real elements instead of racing them.
 */
async function waitForHeaderReady(page: Page): Promise<void> {
  const heading = page.getByRole("heading", { level: 1 }).first();
  await expect(heading).toBeVisible();
  await expect(page.locator('[data-slot="page-chrome-badge"]')).toBeVisible();
  await expect(page.locator('[data-slot="app-header"] [data-slot="page-actions"] button').first()).toBeVisible();
}

/** Overwrite the real header with a worst-case payload and clone action controls to force wrap. */
function stressHeader(page: Page): Promise<void> {
  return page.evaluate(
    ({ longTitle, longBadge, clones }: { longTitle: string; longBadge: string; clones: number }) => {
      const h1 = document.querySelector("h1");
      if (!h1) throw new Error("Global page title not found");
      const root = h1.closest('[data-slot="app-header"]');
      const actions = root?.querySelector('[data-slot="page-actions"]');
      if (!actions) throw new Error("Global page actions not found");

      h1.textContent = longTitle;
      const badgeEl = h1.nextElementSibling;
      if (badgeEl) badgeEl.textContent = longBadge;

      const originals = Array.from(actions.querySelectorAll("button"));
      const seed = originals[0];
      if (seed) {
        for (let i = 0; i < clones; i += 1) actions.append(seed.cloneNode(true));
      }
      for (const button of originals) button.setAttribute("data-pwa66-original", "true");
    },
    { longTitle: LONG_TITLE, longBadge: LONG_BADGE, clones: ACTION_CLONES },
  );
}

/** Measure the live header from the rendered DOM. */
function readGeometry(page: Page): Promise<HeaderGeometry> {
  return page.evaluate((): HeaderGeometry => {
    const toRect = (element: Element): Rect => {
      const rect = element.getBoundingClientRect();
      return {
        left: rect.left,
        right: rect.right,
        top: rect.top,
        bottom: rect.bottom,
        width: rect.width,
        height: rect.height,
      };
    };

    const h1 = document.querySelector("h1");
    if (!h1) throw new Error("Global page title not found");
    const root = h1.closest<HTMLElement>('[data-slot="app-header"]');
    const actions = root?.querySelector('[data-slot="page-actions"]');
    if (!root || !actions) throw new Error("Global page chrome not found");

    // One rect per rendered line box: the decisive "did it wrap or clip" signal.
    const range = document.createRange();
    range.selectNodeContents(h1);
    const lineBottoms = Array.from(range.getClientRects(), (rect) => rect.bottom);
    const titleRect = toRect(h1);

    const badgeEl = h1.nextElementSibling;
    const badge = badgeEl?.matches('[data-slot="page-chrome-badge"]') ? toRect(badgeEl) : null;

    const actionRects = Array.from(actions.querySelectorAll("button"), toRect);
    const header = toRect(root);
    const originals = Array.from(
      actions.querySelectorAll<HTMLButtonElement>("[data-pwa66-original]"),
    );

    return {
      header,
      headerClientWidth: root.clientWidth,
      headerScrollWidth: root.scrollWidth,
      title: titleRect,
      titleClientWidth: h1.clientWidth,
      titleScrollWidth: h1.scrollWidth,
      titleClientHeight: h1.clientHeight,
      titleScrollHeight: h1.scrollHeight,
      titleLineCount: lineBottoms.length,
      titleLastLineBottom: lineBottoms.length > 0 ? Math.max(...lineBottoms) : titleRect.bottom,
      badge,
      actionRects,
      actionRowCount: new Set(actionRects.map((rect) => Math.round(rect.top))).size,
      actionsWithinHeader: actionRects.every(
        (rect) => rect.left >= header.left - 1 && rect.right <= header.right + 1,
      ),
      originalControlsUsable: originals.every((button) => {
        const rect = button.getBoundingClientRect();
        if (button.disabled || rect.width <= 0 || rect.height <= 0) return false;
        const hit = document.elementFromPoint(
          (rect.left + rect.right) / 2,
          (rect.top + rect.bottom) / 2,
        );
        return hit !== null && button.contains(hit);
      }),
      pageClientWidth: document.documentElement.clientWidth,
      pageScrollWidth: document.documentElement.scrollWidth,
    };
  });
}

/** Capture the live class names so a hunk toggle can be undone. */
function snapshotClasses(page: Page): Promise<ClassSnapshot> {
  return page.evaluate((): ClassSnapshot => {
    const h1 = document.querySelector("h1");
    if (!h1) throw new Error("Global page title not found");
    const titleRow = h1.parentElement;
    const root = h1.closest('[data-slot="app-header"]');
    const actions = root?.querySelector('[data-slot="page-actions"]');
    if (!titleRow || !actions) throw new Error("Global page chrome not found");
    return { title: h1.className, titleRow: titleRow.className, actions: actions.className };
  });
}

/** Apply the pre-fix class for one hunk to the live DOM (never to the shared source). */
function applyLegacyHunk(page: Page, hunk: Hunk): Promise<void> {
  return page.evaluate((target: Hunk) => {
    const h1 = document.querySelector("h1");
    if (!h1) throw new Error("Global page title not found");
    const titleRow = h1.parentElement;
    const root = h1.closest('[data-slot="app-header"]');
    const actions = root?.querySelector('[data-slot="page-actions"]');
    if (!titleRow || !actions) throw new Error("Global page chrome not found");

    if (target === "title") {
      h1.classList.remove("min-w-0", "break-words");
      h1.classList.add("truncate");
    } else if (target === "titleRow") {
      titleRow.classList.remove("flex-wrap");
      titleRow.classList.add("flex-nowrap");
    } else {
      actions.className = "flex shrink-0 items-center gap-2";
    }
  }, hunk);
}

function restoreClasses(page: Page, snapshot: ClassSnapshot): Promise<void> {
  return page.evaluate((classes: ClassSnapshot) => {
    const h1 = document.querySelector("h1");
    if (!h1) throw new Error("Global page title not found");
    const titleRow = h1.parentElement;
    const root = h1.closest('[data-slot="app-header"]');
    const actions = root?.querySelector('[data-slot="page-actions"]');
    if (!titleRow || !actions) throw new Error("Global page chrome not found");
    h1.className = classes.title;
    titleRow.className = classes.titleRow;
    actions.className = classes.actions;
  }, snapshot);
}

/** The long unbroken title must break across lines instead of clipping to one. */
const titleWraps = (geometry: HeaderGeometry): boolean =>
  geometry.titleLineCount >= 2 && geometry.titleScrollWidth <= geometry.titleClientWidth + 1;

/** The badge must move below the last title line once the title fills the row. */
const badgeDropsBelowTitle = (geometry: HeaderGeometry): boolean =>
  geometry.badge !== null && geometry.badge.top >= geometry.titleLastLineBottom - 1;

/** The cloned action controls must wrap into multiple rows and stay inside the header. */
const actionsWrapWithinHeader = (geometry: HeaderGeometry): boolean =>
  geometry.actionRowCount >= 2 && geometry.actionsWithinHeader;

const summary = (geometry: HeaderGeometry): string =>
  `lines=${geometry.titleLineCount} badgeTop=${geometry.badge?.top ?? "null"} ` +
  `lastLineBottom=${geometry.titleLastLineBottom} actionRows=${geometry.actionRowCount} ` +
  `within=${geometry.actionsWithinHeader} header=${geometry.header.left}..${geometry.header.right}`;

for (const route of ROUTES) {
  for (const viewport of VIEWPORTS) {
    test(`${route} PageHeader fits at ${viewport.width}×${viewport.height}`, async ({
      appPage: page,
    }) => {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await page.goto(route);
      await expectRoutedPage(page);
      await waitForHeaderReady(page);

      const geometry = await readGeometry(page);

      expect(geometry.title.width).toBeGreaterThan(0);
      expect(geometry.titleScrollWidth).toBeLessThanOrEqual(geometry.titleClientWidth + 1);
      expect(geometry.titleScrollHeight).toBeLessThanOrEqual(geometry.titleClientHeight + 1);
      expect(geometry.header.left).toBeGreaterThanOrEqual(0);
      expect(geometry.header.right).toBeLessThanOrEqual(viewport.width + 1);
      // Header-specific overflow: the header's own content must fit its box.
      expect(geometry.headerScrollWidth).toBeLessThanOrEqual(geometry.headerClientWidth + 1);
      expect(geometry.badge).not.toBeNull();
      expect(geometry.badge!.width).toBeGreaterThan(0);
      expect(geometry.badge!.right).toBeLessThanOrEqual(viewport.width + 1);
      expect(geometry.actionRects.length).toBeGreaterThan(0);
      for (const rect of geometry.actionRects) {
        expect(rect.left).toBeGreaterThanOrEqual(0);
        expect(rect.right).toBeLessThanOrEqual(viewport.width + 1);
        expect(rect.bottom).toBeLessThanOrEqual(viewport.height);
      }
      // Unstressed content keeps the historical layout: one row on desktop,
      // stacked below the title on mobile.
      if (viewport.width === 1440) {
        expect(geometry.actionRects[0]!.top).toBeLessThan(geometry.title.top + 30);
      } else {
        expect(geometry.actionRects[0]!.top).toBeGreaterThan(geometry.title.top + 20);
      }
      // Document-wide guard: the page itself must not scroll horizontally. It is
      // weaker than the header-specific checks above — a clipping ancestor can
      // hold the page width fixed while the header content still overflows its
      // box — so it supplements rather than replaces them.
      expect(geometry.pageScrollWidth).toBeLessThanOrEqual(geometry.pageClientWidth + 1);
    });
  }
}

for (const route of ROUTES) {
  for (const viewport of VIEWPORTS) {
    test(`${route} PageHeader stress-wraps at ${viewport.width}×${viewport.height}`, async ({
      appPage: page,
    }) => {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await page.goto(route);
      await expectRoutedPage(page);
      await waitForHeaderReady(page);

      await stressHeader(page);
      const snapshot = await snapshotClasses(page);

      const current = await readGeometry(page);
      expect(titleWraps(current), `title must wrap — ${summary(current)}`).toBe(true);
      expect(
        badgeDropsBelowTitle(current),
        `badge must drop below a long title — ${summary(current)}`,
      ).toBe(true);
      expect(
        actionsWrapWithinHeader(current),
        `cloned actions must wrap inside the header — ${summary(current)}`,
      ).toBe(true);
      expect(current.originalControlsUsable, "original action controls stay hit-testable").toBe(true);
      // The same wrap, read from scroll geometry: the title content does not
      // spill past its own width. (`scrollHeight` cannot exceed `clientHeight`
      // here — a wrapping block grows to fit its lines — so the line-box count
      // above is the decisive wrap measure.)
      expect(current.titleScrollWidth).toBeLessThanOrEqual(current.titleClientWidth + 1);
      expect(current.header.left).toBeGreaterThanOrEqual(0);
      expect(current.header.right).toBeLessThanOrEqual(viewport.width + 1);
      expect(current.headerScrollWidth).toBeLessThanOrEqual(current.headerClientWidth + 1);
      expect(current.pageScrollWidth).toBeLessThanOrEqual(current.pageClientWidth + 1);

      // Each hunk is load-bearing: reverting it in the live DOM makes its own
      // geometry predicate false while the others remain in place.
      await applyLegacyHunk(page, "title");
      const legacyTitle = await readGeometry(page);
      expect(
        titleWraps(legacyTitle),
        `pre-fix truncate must fail the title-wrap predicate — ${summary(legacyTitle)}`,
      ).toBe(false);
      await restoreClasses(page, snapshot);

      await applyLegacyHunk(page, "titleRow");
      const legacyTitleRow = await readGeometry(page);
      expect(
        badgeDropsBelowTitle(legacyTitleRow),
        `pre-fix non-wrapping title row must fail the badge predicate — ${summary(legacyTitleRow)}`,
      ).toBe(false);
      await restoreClasses(page, snapshot);

      await applyLegacyHunk(page, "actions");
      const legacyActions = await readGeometry(page);
      expect(
        actionsWrapWithinHeader(legacyActions),
        `pre-fix non-wrapping action row must fail the actions predicate — ${summary(legacyActions)}`,
      ).toBe(false);
      await restoreClasses(page, snapshot);
    });
  }
}
