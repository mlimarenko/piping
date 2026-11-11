import type { Page } from "@playwright/test";

import { expect, test, expectRoutedPage } from "./fixtures";

/** Check rendered action bounds and hit targets with both image values set. */

test.use({ locale: "ru-RU" });

const PNG_BYTES = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  "base64",
);

const VIEWPORTS = [
  { label: "desktop", width: 1440, height: 900 },
  { label: "mobile-390", width: 390, height: 844 },
  { label: "mobile-320", width: 320, height: 700 },
] as const;

const CLEAR_LABEL = "Очистить";
const SAVE_LABEL = "Сохранить";

interface Rect {
  left: number;
  right: number;
  top: number;
  bottom: number;
  width: number;
}

interface ButtonGeometry extends Rect {
  text: string;
  probes: Array<{ x: number; isSelf: boolean; hit: string }>;
}

interface FieldGeometry {
  label: string | null;
  left: number;
  right: number;
  width: number;
  buttons: ButtonGeometry[];
}

interface AssetsGeometry {
  innerWidth: number;
  card: Rect;
  fields: FieldGeometry[];
}

/** Populate the draft through file inputs; persistence uses the demo API. */
async function openWithBothAssets(page: Page): Promise<void> {
  await page.goto("/administration/branding");
  await expectRoutedPage(page);
  const inputs = page.locator('input[type="file"]');
  await expect(inputs).toHaveCount(2);
  for (let index = 0; index < 2; index += 1) {
    await inputs
      .nth(index)
      .setInputFiles({ name: `asset-${index}.png`, mimeType: "image/png", buffer: PNG_BYTES });
  }
  await expect(page.getByRole("button", { name: CLEAR_LABEL })).toHaveCount(2);
}

/** The assets card: the only SurfaceCard holding the two file inputs. */
function assetsCard(page: Page) {
  return page
    .locator('[data-slot="surface-card"]')
    .filter({ has: page.locator('input[type="file"]') });
}

/** Measure every action button and its card from the rendered DOM. */
function readGeometry(page: Page): Promise<AssetsGeometry> {
  return page.evaluate((): AssetsGeometry => {
    const inputs = Array.from(document.querySelectorAll<HTMLInputElement>('input[type="file"]'));
    if (inputs.length === 0) throw new Error("no file inputs rendered");
    const cardEl = inputs[0]!.closest('[data-slot="surface-card"]');
    if (!cardEl) throw new Error("assets card not found");
    // Horizontal edges do not move under vertical scrolling, which is all the
    // probe step below performs.
    const cardRect = cardEl.getBoundingClientRect();

    const fields = inputs.map((input): FieldGeometry => {
      const fieldEl = input.parentElement;
      if (!fieldEl) throw new Error("image field root not found");
      // Bring the field into the vertical viewport so elementFromPoint can
      // resolve its buttons. Scroll the field root with `inline: "nearest"`: the
      // root fits its card, so no horizontal scroll happens and the edges under
      // test never move.
      fieldEl.scrollIntoView({ block: "center", inline: "nearest" });
      const fieldRect = fieldEl.getBoundingClientRect();
      const buttons = Array.from(fieldEl.querySelectorAll("button")).map((button): ButtonGeometry => {
        const rect = button.getBoundingClientRect();
        const midY = (rect.top + rect.bottom) / 2;
        // Left edge, centre and right edge: a button covered by a neighbouring
        // column fails the centre/right probe even when its own bounding rect is
        // still reported.
        const probes = [rect.left + 2, (rect.left + rect.right) / 2, rect.right - 2].map((x) => {
          const hit = document.elementFromPoint(x, midY);
          const isSelf = hit !== null && button.contains(hit);
          return {
            x,
            isSelf,
            hit: isSelf ? "self" : (hit?.textContent ?? "null").trim().slice(0, 24),
          };
        });
        return {
          left: rect.left,
          right: rect.right,
          top: rect.top,
          bottom: rect.bottom,
          width: rect.width,
          text: (button.textContent ?? "").trim(),
          probes,
        };
      });
      return {
        label: fieldEl.querySelector("span")?.textContent ?? null,
        left: fieldRect.left,
        right: fieldRect.right,
        width: fieldRect.width,
        buttons,
      };
    });

    return {
      innerWidth: window.innerWidth,
      card: {
        left: cardRect.left,
        right: cardRect.right,
        top: cardRect.top,
        bottom: cardRect.bottom,
        width: cardRect.width,
      },
      fields,
    };
  });
}

/** A button is usable only when it is painted inside its card, inside the
 * viewport, and owns every hit point across its own width. */
function isFullyVisible(button: ButtonGeometry, card: Rect, innerWidth: number): boolean {
  return (
    button.width > 0 &&
    button.left >= card.left - 1 &&
    button.right <= card.right + 1 &&
    button.left >= -1 &&
    button.right <= innerWidth + 1 &&
    button.probes.every((probe) => probe.isSelf)
  );
}

function describeButton(field: FieldGeometry, button: ButtonGeometry, card: Rect, innerWidth: number): string {
  const bad = button.probes.filter((probe) => !probe.isSelf).map((probe) => `x=${Math.round(probe.x)}→${probe.hit}`);
  return (
    `${field.label}/${button.text} [${button.left.toFixed(0)}..${button.right.toFixed(0)}] ` +
    `card=[${card.left.toFixed(0)}..${card.right.toFixed(0)}] viewport=${innerWidth} ` +
    `bad-probes=${bad.join(",") || "none"}`
  );
}

function assertAllVisible(geometry: AssetsGeometry, label: string): void {
  for (const field of geometry.fields) {
    for (const button of field.buttons) {
      expect(
        isFullyVisible(button, geometry.card, geometry.innerWidth),
        `${label}: ${describeButton(field, button, geometry.card, geometry.innerWidth)}`,
      ).toBe(true);
    }
  }
}

test("keeps both asset actions visible, hit-testable and keyboard-operable from 320px up", async ({ page }) => {
  for (const viewport of VIEWPORTS) {
    await page.setViewportSize({ width: viewport.width, height: viewport.height });
    await openWithBothAssets(page);
    assertAllVisible(await readGeometry(page), viewport.label);
  }

  await page.setViewportSize({ width: 390, height: 844 });
  await openWithBothAssets(page);
  const faviconClear = assetsCard(page).getByRole("button", { name: CLEAR_LABEL }).nth(1);
  await expect(faviconClear).toBeInViewport();
  await faviconClear.focus();
  await expect(faviconClear).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("button", { name: CLEAR_LABEL })).toHaveCount(1);
  await expect(assetsCard(page).getByRole("img", { name: "Фавикон" })).toHaveCount(0);
});

test("preserves the upload and clear value contract through Save", async ({ page }) => {
  await openWithBothAssets(page);
  const hasValue = (value: string | null) => typeof value === "string" && value.length > 0;

  // Uploaded values reach the API unchanged.
  await page.getByRole("button", { name: SAVE_LABEL }).click();
  await expect
    .poll(async () => {
      const branding = await readBranding(page);
      return [hasValue(branding.logo_data_url), hasValue(branding.favicon_data_url)];
    })
    .toEqual([true, true]);

  // Clearing the favicon with the keyboard is persisted as an explicit null,
  // and the logo value is left untouched.
  const faviconClear = assetsCard(page).getByRole("button", { name: CLEAR_LABEL }).nth(1);
  await faviconClear.focus();
  await page.keyboard.press("Enter");
  await page.getByRole("button", { name: SAVE_LABEL }).click();
  await expect
    .poll(async () => {
      const branding = await readBranding(page);
      return [branding.favicon_data_url, hasValue(branding.logo_data_url)];
    })
    .toEqual([null, true]);

  // Status notices must not block Save, but their dismissal remains interactive.
  const notice = page.locator('[data-sonner-toast][data-front="true"]');
  await expect(notice).toBeVisible();
  const noticeCount = await page.locator("[data-sonner-toast]").count();
  await notice.getByRole("button", { name: /close toast/i }).click();
  await expect.poll(() => page.locator("[data-sonner-toast]").count()).toBeLessThan(noticeCount);
});

test("toast controls respect Sonner stack visibility without blocking underlying actions", async ({ page }) => {
  await page.goto("/administration/branding");
  await expectRoutedPage(page);
  const toasts = page.locator("[data-sonner-toast]");
  for (let count = 1; count <= 4; count++) {
    await page.getByRole("button", { name: SAVE_LABEL }).click();
    await expect(toasts).toHaveCount(count);
  }
  await expect(toasts.nth(3)).toHaveAttribute("data-visible", "false");
  await expect(toasts.nth(3)).toHaveAttribute("data-mounted", "true");

  // Stress Sonner's real rendered DOM/CSS contract, including short-lived exit
  // states. Keep geometry fixed so only interaction eligibility changes.
  // No current app notification supplies action/cancel; native Sonner-shaped
  // buttons exercise those descendant controls under the same rendered styles.
  const results = await page.evaluate(() => {
    const seed = document.querySelector<HTMLElement>('[data-sonner-toast][data-visible="false"]')!;
    const notice = seed.cloneNode(true) as HTMLElement;
    notice.querySelector("[data-icon]")?.remove();
    notice.querySelector("[data-title]")!.textContent = "Toast controls";
    Object.assign(notice.style, { position: "fixed", left: "25px", right: "auto", top: "160px", bottom: "auto", width: "330px", transform: "none", transition: "none", zIndex: "999999" });
    for (const type of ["action", "cancel"]) {
      const button = document.createElement("button");
      button.dataset.button = "true";
      button.setAttribute(`data-${type}`, "true");
      button.textContent = type;
      notice.append(button);
    }
    seed.parentElement!.append(notice);
    const underlay = document.createElement("button");
    underlay.textContent = "Underlying action";
    Object.assign(underlay.style, { position: "fixed", width: "16px", height: "16px", padding: "0", zIndex: "999998" });
    document.body.append(underlay);
    let underlyingClicks = 0;
    underlay.addEventListener("click", () => underlyingClicks++);
    const controls = [
      { name: "close", element: notice.querySelector<HTMLElement>("[data-close-button]")!, clicks: 0 },
      { name: "action", element: notice.querySelector<HTMLElement>("[data-action]")!, clicks: 0 },
      { name: "cancel", element: notice.querySelector<HTMLElement>("[data-cancel]")!, clicks: 0 },
      { name: "body", element: notice.querySelector<HTMLElement>("[data-content]")!, clicks: 0 },
    ];
    for (const control of controls) control.element.addEventListener("click", () => control.clicks++);
    const states = [
      { name: "unmounted front", mounted: false, visible: true, removed: false, front: true, expanded: false, interactive: false },
      { name: "unmounted expanded", mounted: false, visible: true, removed: false, front: false, expanded: true, interactive: false },
      { name: "hidden fourth", mounted: true, visible: false, removed: false, front: false, expanded: true, interactive: false },
      { name: "removed front", mounted: true, visible: true, removed: true, front: true, expanded: false, interactive: false },
      { name: "removed expanded", mounted: true, visible: true, removed: true, front: false, expanded: true, interactive: false },
      { name: "collapsed non-front", mounted: true, visible: true, removed: false, front: false, expanded: false, interactive: false },
      { name: "active front", mounted: true, visible: true, removed: false, front: true, expanded: false, interactive: true },
      { name: "active expanded", mounted: true, visible: true, removed: false, front: false, expanded: true, interactive: true },
    ];
    const probes = states.flatMap(state => {
      notice.dataset.mounted = String(state.mounted);
      notice.dataset.visible = String(state.visible);
      notice.dataset.removed = String(state.removed);
      notice.dataset.front = String(state.front);
      notice.dataset.expanded = String(state.expanded);
      return controls.map(control => {
        const rect = control.element.getBoundingClientRect();
        const x = rect.left + rect.width / 2;
        const y = rect.top + rect.height / 2;
        underlay.style.left = `${x - 8}px`;
        underlay.style.top = `${y - 8}px`;
        const hit = document.elementFromPoint(x, y);
        const beforeControl = control.clicks;
        const beforeUnderlay = underlyingClicks;
        hit?.closest<HTMLButtonElement>("button")?.click();
        return {
          state: state.name, control: control.name,
          expectControl: state.interactive && control.name !== "body",
          hitsControl: hit !== null && control.element.contains(hit),
          hitsUnderlay: hit === underlay,
          controlClicks: control.clicks - beforeControl,
          underlyingClicks: underlyingClicks - beforeUnderlay,
        };
      });
    });
    notice.remove();
    underlay.remove();
    return probes;
  });
  for (const probe of results) {
    const context = `${probe.state}: ${probe.control}`;
    expect(probe.hitsControl, context).toBe(probe.expectControl);
    expect(probe.hitsUnderlay, context).toBe(!probe.expectControl);
    expect(probe.controlClicks, context).toBe(probe.expectControl ? 1 : 0);
    expect(probe.underlyingClicks, context).toBe(probe.expectControl ? 0 : 1);
  }
  const active = page.locator('[data-sonner-toast][data-mounted="true"][data-visible="true"][data-removed="false"][data-front="true"]');
  await active.getByRole("button", { name: /close toast/i }).click();
  await expect(toasts).toHaveCount(3);
});

/** Read the stored branding through the demo API the UI itself uses. */
async function readBranding(page: Page) {
  const result = await page.evaluate(async () => {
    const response = await fetch("/api/v1/instance/branding", { method: "GET" });
    return { ok: response.ok, body: await response.text() };
  });
  expect(result.ok).toBe(true);
  return JSON.parse(result.body) as { logo_data_url: string | null; favicon_data_url: string | null };
}
