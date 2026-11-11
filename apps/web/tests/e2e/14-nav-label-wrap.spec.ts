import type { Page } from "@playwright/test";

import { expect, test, expectRoutedPage, waitForConnectionsPageReady } from "./fixtures";

/**
 * Navigation labels must be fully readable (PWA-65).
 *
 * Before this fix every navigation label carried `truncate`: inside the 208px
 * rail the Russian "Конфигурации подключений" clipped to "Конфигурации
 * подклю…" with no way to read the rest. The canonical rule — one shared
 * source of truth in `src/shared/ui-shared/navLabel.ts` — is that a navigation
 * label wraps to a second line (`break-words`, capped by `line-clamp-2`), the
 * full text stays on `title` and in the accessible name, and the leading icon
 * stays on the first line. The same rule covers the sidebar sections, the
 * collapsed-rail dropdown, the mobile drawer (which reuses AppSidebar) and the
 * top-bar page title.
 *
 * These tests read the real rendered DOM. The "fits" cases measure the shipped
 * label at 1440×900 (the fixed rail) and 390×844 (the mobile drawer); the
 * "stress" cases overwrite the label with a longer phrase, because at drawer
 * width the shipped label happens to fit on one line. Each stress case applies
 * the pre-fix `truncate` class to the live DOM — never to the shared source —
 * and asserts the predicate goes false, which proves the wrap is load-bearing
 * rather than incidentally present.
 */
test.use({ locale: "ru-RU" });

const SHIPPED_LABEL = "Конфигурации подключений";
const SIDEBAR_STRESS_LABEL =
  "Конфигурации подключений очень длинное название раздела навигации";
const TOPBAR_STRESS_LABEL =
  "Рабочее пространство подключений и конфигурации и ещё немного текста для проверки";

const VIEWPORTS = [
  { label: "desktop", width: 1440, height: 900, drawer: false },
  { label: "mobile", width: 390, height: 844, drawer: true },
] as const;

type Viewport = (typeof VIEWPORTS)[number];

interface LabelGeometry {
  text: string;
  className: string;
  title: string | null;
  clientWidth: number;
  scrollWidth: number;
  lineCount: number;
  firstLineTop: number;
  firstLineBottom: number;
  iconTop: number | null;
  iconBottom: number | null;
}

/** A navigation label renders its full text without a horizontal clip. */
const notClipped = (geometry: LabelGeometry): boolean =>
  geometry.scrollWidth <= geometry.clientWidth + 1;

/** It breaks onto a second line instead of staying on one. */
const wraps = (geometry: LabelGeometry): boolean => geometry.lineCount >= 2;

/** The leading icon sits inside the first text line, not floating between lines. */
const iconOnFirstLine = (geometry: LabelGeometry): boolean =>
  geometry.iconTop !== null &&
  geometry.iconBottom !== null &&
  geometry.iconTop >= geometry.firstLineTop - 1 &&
  geometry.iconBottom <= geometry.firstLineBottom + 1;

const summary = (geometry: LabelGeometry): string =>
  `text=${geometry.text} lines=${geometry.lineCount} ` +
  `client=${geometry.clientWidth} scroll=${geometry.scrollWidth} ` +
  `firstLine=${geometry.firstLineTop.toFixed(1)}..${geometry.firstLineBottom.toFixed(1)} ` +
  `icon=${geometry.iconTop === null ? "null" : `${geometry.iconTop.toFixed(1)}..${geometry.iconBottom!.toFixed(1)}`} ` +
  `class="${geometry.className}" title=${geometry.title === null ? "null" : "set"}`;

async function openNav(page: Page, viewport: Viewport): Promise<void> {
  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  await page.goto("/connections/config");
  await expectRoutedPage(page);
  if (viewport.drawer) {
    // The mobile shell hides the rail until the top-bar toggle opens the drawer.
    await page.getByRole("button", { name: "Переключить боковую панель" }).click();
  }
  await expect(page.locator('nav a[href="/connections/config"]')).toBeVisible();
}

/**
 * Measure the connection-config label and its leading icon. `clientHeight /
 * lineHeight` is the line count: a `line-clamp-2` label is a `-webkit-box`, and
 * a Range over its contents is not guaranteed to yield one rect per line in
 * every engine, while the box height is exact.
 */
function readSidebarLabel(page: Page): Promise<LabelGeometry> {
  return page.evaluate((): LabelGeometry => {
    const link = document.querySelector('nav a[href="/connections/config"]');
    if (!link) throw new Error("connection-config nav link not found");
    const label = link.querySelector("span");
    if (!label) throw new Error("nav label span not found");
    const icon = link.querySelector("svg");

    const labelRect = label.getBoundingClientRect();
    const computed = getComputedStyle(label);
    const fontSize = parseFloat(computed.fontSize);
    const lineHeight = computed.lineHeight === "normal"
      ? fontSize * 1.5
      : parseFloat(computed.lineHeight);
    const iconRect = icon?.getBoundingClientRect();

    return {
      text: label.textContent ?? "",
      className: label.className,
      title: label.getAttribute("title"),
      clientWidth: label.clientWidth,
      scrollWidth: label.scrollWidth,
      lineCount: Math.max(1, Math.round(labelRect.height / lineHeight)),
      firstLineTop: labelRect.top,
      firstLineBottom: labelRect.top + lineHeight,
      iconTop: iconRect ? iconRect.top : null,
      iconBottom: iconRect ? iconRect.bottom : null,
    };
  });
}

/** Overwrite the live sidebar label with the worst-case payload. */
async function stressSidebarLabel(page: Page): Promise<void> {
  await page.evaluate((text: string) => {
    const link = document.querySelector('nav a[href="/connections/config"]');
    const label = link?.querySelector("span");
    if (!label) throw new Error("nav label span not found");
    label.textContent = text;
  }, SIDEBAR_STRESS_LABEL);
}

/** Apply the pre-fix class to the live label, measure, then restore it. */
async function withLegacyTruncate(
  page: Page,
  read: () => Promise<LabelGeometry>,
): Promise<LabelGeometry> {
  const original = await page.evaluate(() => {
    const link = document.querySelector('nav a[href="/connections/config"]');
    const label = link?.querySelector("span");
    if (!label) throw new Error("nav label span not found");
    const className = label.className;
    label.className = "truncate";
    return className;
  });
  const geometry = await read();
  await page.evaluate((className: string) => {
    const link = document.querySelector('nav a[href="/connections/config"]');
    const label = link?.querySelector("span");
    if (!label) throw new Error("nav label span not found");
    label.className = className;
  }, original);
  return geometry;
}

function readTopBarTitle(page: Page): Promise<LabelGeometry> {
  return page.evaluate((): LabelGeometry => {
    const title = document.querySelector('[data-slot="app-header-title"]');
    if (!title) throw new Error("top-bar title span not found");
    const rect = title.getBoundingClientRect();
    const computed = getComputedStyle(title);
    const fontSize = parseFloat(computed.fontSize);
    const lineHeight = computed.lineHeight === "normal"
      ? fontSize * 1.5
      : parseFloat(computed.lineHeight);
    return {
      text: title.textContent ?? "",
      className: title.className,
      title: title.getAttribute("title"),
      clientWidth: title.clientWidth,
      scrollWidth: title.scrollWidth,
      lineCount: Math.max(1, Math.round(rect.height / lineHeight)),
      firstLineTop: rect.top,
      firstLineBottom: rect.top + lineHeight,
      iconTop: null,
      iconBottom: null,
    };
  });
}

for (const viewport of VIEWPORTS) {
  test(`${viewport.label}: shipped nav label is not truncated`, async ({ appPage: page }) => {
    await openNav(page, viewport);
    const geometry = await readSidebarLabel(page);

    expect(geometry.text).toBe(SHIPPED_LABEL);
    expect(geometry.className, "nav labels must use the shared wrap rule").not.toContain("truncate");
    expect(geometry.className).toContain("break-words");
    expect(geometry.title, "the full label must stay reachable via title").toBe(SHIPPED_LABEL);
    expect(notClipped(geometry), `shipped label must not clip — ${summary(geometry)}`).toBe(true);
    if (viewport.label === "desktop") {
      // The rail is fixed at w-52 (208px); this shipped label demonstrably needs
      // a second line there, which is the reported defect.
      expect(wraps(geometry), `shipped label must wrap in the rail — ${summary(geometry)}`).toBe(true);
    }
  });

  test(`${viewport.label}: long nav label wraps and keeps the icon on the first line`, async ({
    appPage: page,
  }) => {
    await openNav(page, viewport);
    await stressSidebarLabel(page);

    const geometry = await readSidebarLabel(page);
    expect(wraps(geometry), `label must wrap — ${summary(geometry)}`).toBe(true);
    expect(notClipped(geometry), `label must not clip — ${summary(geometry)}`).toBe(true);
    expect(iconOnFirstLine(geometry), `icon must stay on line 1 — ${summary(geometry)}`).toBe(true);

    // Load-bearing: the pre-fix class must fail the same predicates.
    const legacy = await withLegacyTruncate(page, () => readSidebarLabel(page));
    expect(wraps(legacy), `pre-fix truncate must fail the wrap predicate — ${summary(legacy)}`).toBe(false);
    expect(notClipped(legacy), `pre-fix truncate must fail the fit predicate — ${summary(legacy)}`).toBe(false);
  });
}

test("top-bar page title wraps instead of clipping", async ({ appPage: page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/connections");
  await waitForConnectionsPageReady(page);

  await page.evaluate((text: string) => {
    const title = document.querySelector('[data-slot="app-header-title"]');
    if (!title) throw new Error("top-bar title span not found");
    title.textContent = text;
  }, TOPBAR_STRESS_LABEL);

  const geometry = await readTopBarTitle(page);
  expect(geometry.className).not.toContain("truncate");
  expect(wraps(geometry), `title must wrap — ${summary(geometry)}`).toBe(true);
  expect(notClipped(geometry), `title must not clip — ${summary(geometry)}`).toBe(true);
});
