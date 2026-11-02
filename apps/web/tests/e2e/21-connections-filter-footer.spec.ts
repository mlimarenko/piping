import type { Locator, Page } from "@playwright/test";

import { captureLayoutThemes, expect, test, waitForConnectionsPageReady } from "./fixtures";

/**
 * ConnectionsFilters footer must stay inside the dialog and stay tappable.
 *
 * Before this fix the footer row was `flex ... justify-between` with a
 * `shrink-0` action group of three fixed `h-8` buttons. At 320px with the long
 * Russian "Сбросить черновик" the group was wider than the dialog, and the
 * dialog's `overflow-hidden` simply cut the trailing "Применить" off the right
 * edge — the primary action was unreachable. The canonical rule is that the
 * footer wraps/stacks at phone widths, every action is at least a 44px touch
 * target while the dialog is open, and the desktop row stays dense (32px).
 *
 * These tests measure the real rendered DOM: the dialog is the containment
 * boundary, so a button whose box escapes it (or the viewport) fails.
 */

interface Viewport {
  name: string;
  width: number;
  height: number;
  locale: string;
  touch: boolean;
}

const VIEWPORTS: readonly Viewport[] = [
  { name: "phone-320-ru", width: 320, height: 568, locale: "ru-RU", touch: true },
  { name: "phone-390-ru", width: 390, height: 844, locale: "ru-RU", touch: true },
  { name: "phone-390-en", width: 390, height: 844, locale: "en-US", touch: true },
  { name: "desktop-1440", width: 1440, height: 900, locale: "en-US", touch: false },
];

// The trigger's accessible name grows to "Filters (N active)" once a filter is
// applied, so the matcher is a prefix rather than an exact name.
const FILTER_TRIGGER = /^(Filters|Фильтры)/;
const RESET = /^(Reset draft|Сбросить черновик)$/;
const CANCEL = /^(Cancel|Отмена)$/;
const APPLY = /^(Apply|Применить)$/;
const CLEAR_ALL = /^(Clear all|Очистить все)$/;
const CLOSE = /^(Close|Закрыть)$/;
const SAVE = /^(Save filter|Сохранить фильтр)$/;
// While the draft is empty the Save trigger's `title` — and therefore its
// accessible name — is the "select a filter first" hint; only once the draft
// has a filter does it become the Save label. Match either state.
const SAVE_TRIGGER = /^(Save filter|Сохранить фильтр|Select at least one filter|Выберите хотя бы один фильтр)$/;

interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

async function box(locator: Locator): Promise<Rect> {
  const rect = await locator.boundingBox();
  expect(rect, "rendered action must have native browser geometry").not.toBeNull();
  return rect!;
}

const inside = (child: Rect, parent: Rect): boolean =>
  child.x >= parent.x - 1 &&
  child.y >= parent.y - 1 &&
  child.x + child.width <= parent.x + parent.width + 1 &&
  child.y + child.height <= parent.y + parent.height + 1;

async function openFilters(page: Page): Promise<Locator> {
  await page.getByRole("button", { name: FILTER_TRIGGER }).click();
  const dialog = page.locator('[data-slot="dialog-content"]');
  await expect(dialog).toBeVisible();
  return dialog;
}

/**
 * Open the catalogue filters and assert every footer action is inside the
 * dialog, inside the viewport, and at the height the app's phone/desktop split
 * prescribes: <768px is a phone (`use-mobile`) and gets a 44px touch target,
 * ≥768px is desktop and stays dense (≤40px).
 */
async function assertFooterFits(page: Page, viewport: Viewport): Promise<void> {
  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  await page.goto("/connections");
  await waitForConnectionsPageReady(page);

  const dialog = await openFilters(page);
  const dialogBox = await box(dialog);
  const actions: Array<[string, Rect]> = [
    ["reset", await box(dialog.getByRole("button", { name: RESET }))],
    ["cancel", await box(dialog.getByRole("button", { name: CANCEL }))],
    ["apply", await box(dialog.getByRole("button", { name: APPLY }))],
  ];
  for (const [name, rect] of actions) {
    expect.soft(inside(rect, dialogBox), `${viewport.name} ${name} box ${JSON.stringify(rect)} escapes dialog ${JSON.stringify(dialogBox)}`).toBe(true);
    expect.soft(rect.x + rect.width, `${viewport.name} ${name} right edge inside ${viewport.width}px viewport`).toBeLessThanOrEqual(viewport.width + 1);
    if (viewport.touch) {
      expect.soft(rect.height, `${viewport.name} ${name} touch height below the 768px boundary`).toBeGreaterThanOrEqual(44);
    } else {
      expect.soft(rect.height, `${viewport.name} ${name} dense height at/above the 768px boundary`).toBeLessThanOrEqual(40);
    }
  }
}

for (const viewport of VIEWPORTS) {
  test.describe(`connections filter footer at ${viewport.name}`, () => {
    test.use({ locale: viewport.locale });

    test("actions stay inside the dialog and meet the viewport touch rule", async ({ page }) => {
      await assertFooterFits(page, viewport);
      await captureLayoutThemes(page, process.env.PIP103_APP_PROOF_DIR, `connections-filter-footer-${viewport.name}`);
    });

    test("draft semantics survive reset, apply and cancel", async ({ page }) => {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await page.goto("/connections");
      await waitForConnectionsPageReady(page);

      const dialog = await openFilters(page);
      const reset = dialog.getByRole("button", { name: RESET });
      const apply = dialog.getByRole("button", { name: APPLY });

      // Nothing is dirty on open, so the primary action is inert.
      await expect(apply).toBeDisabled();

      // Choose an option, then clear it again through Reset: the draft returns
      // to the applied selection and Apply is inert once more.
      await dialog.getByRole("button", { name: /^(active|активн)/ }).first().click();
      await expect(apply).toBeEnabled();
      await reset.click();
      await expect(apply).toBeDisabled();

      // Choose again, activate the primary action from the keyboard: the point
      // is hit-testable and the draft is published without a pointer.
      await dialog.getByRole("button", { name: /^(active|активн)/ }).first().click();
      await expect(apply).toBeEnabled();
      await apply.focus();
      await page.keyboard.press("Enter");
      await expect(dialog).toBeHidden();

      // Re-open: the published filter is now the applied selection, so Apply is
      // inert immediately and Cancel cannot resurrect the discarded draft.
      const reopened = await openFilters(page);
      await expect(reopened.getByRole("button", { name: APPLY })).toBeDisabled();
      await reopened.getByRole("button", { name: /^(active|активн)/ }).first().click();
      await expect(reopened.getByRole("button", { name: APPLY })).toBeEnabled();
      await reopened.getByRole("button", { name: CANCEL }).click();
      await expect(reopened).toBeHidden();
      const finalDialog = await openFilters(page);
      await expect(finalDialog.getByRole("button", { name: APPLY })).toBeDisabled();
    });

    test("header clear action does not overlap Close or dismiss the draft", async ({ page }) => {
      await page.setViewportSize({ width: viewport.width, height: viewport.height });
      await page.goto("/connections");
      await waitForConnectionsPageReady(page);

      const dialog = await openFilters(page);
      await dialog.getByRole("button", { name: /^(active|активн)/ }).first().click();
      await dialog.getByRole("button", { name: APPLY }).click();
      await expect(dialog).toBeHidden();

      const reopened = await openFilters(page);
      const clear = reopened.getByRole("button", { name: CLEAR_ALL });
      const clearBox = await box(clear);
      const closeBox = await box(reopened.getByRole("button", { name: CLOSE, exact: true }));
      expect(clearBox.x + clearBox.width).toBeLessThanOrEqual(closeBox.x);
      await clear.click({ position: { x: clearBox.width - 4, y: clearBox.height / 2 } });
      await expect(reopened).toBeVisible();
      await expect(clear).toBeHidden();
      await expect(reopened.getByRole("button", { name: APPLY })).toBeEnabled();
      await reopened.getByRole("button", { name: APPLY }).click();
      await expect(reopened).toBeHidden();

      const cleared = await openFilters(page);
      await expect(cleared.getByRole("button", { name: CLEAR_ALL })).toBeHidden();
      await expect(cleared.getByRole("button", { name: APPLY })).toBeDisabled();
    });
  });
}

/**
 * The independent review's class boundary: the app splits phone from desktop at
 * 768px (`use-mobile`), so the dense 32px footer height must key off `md`, not
 * `sm`. 640–767 is still a phone and must keep the 44px touch target; 768 is the
 * first desktop width and may return to the dense row.
 */
const BOUNDARY_VIEWPORTS: readonly Viewport[] = [
  { name: "phone-640-ru", width: 640, height: 800, locale: "ru-RU", touch: true },
  { name: "phone-767-ru", width: 767, height: 800, locale: "ru-RU", touch: true },
  { name: "desktop-768-ru", width: 768, height: 900, locale: "ru-RU", touch: false },
];

for (const viewport of BOUNDARY_VIEWPORTS) {
  test.describe(`connections filter footer boundary at ${viewport.name}`, () => {
    test.use({ locale: viewport.locale });

    test("dense height flips at the app's 768px use-mobile boundary", async ({ page }) => {
      await assertFooterFits(page, viewport);
      await captureLayoutThemes(page, process.env.PIP103_APP_PROOF_DIR, `connections-filter-footer-boundary-${viewport.name}`);
    });
  });
}

/**
 * The nested Save-filter dialog is 288px wide inside the 320px Cloud iframe.
 * Its Cancel/Save actions must stay inside it and be 44px touch targets at that
 * width, and dismissing it from the keyboard must leave the unsaved draft
 * intact.
 */
test.describe("connections filter save dialog at the 320px/288px Cloud width", () => {
  test.use({ locale: "ru-RU" });

  test("save dialog fits 288px and keyboard cancel keeps the draft", async ({ page }) => {
    await page.setViewportSize({ width: 320, height: 568 });
    await page.goto("/connections");
    await waitForConnectionsPageReady(page);

    const dialog = await openFilters(page);
    const apply = dialog.getByRole("button", { name: APPLY });
    const saveTrigger = dialog.getByRole("button", { name: SAVE_TRIGGER });

    // The Save action carries the draft, so it is inert until a filter is chosen.
    await expect(saveTrigger).toBeDisabled();
    await dialog.getByRole("button", { name: /^(active|активн)/ }).first().click();
    await expect(saveTrigger).toBeEnabled();
    await saveTrigger.click();

    const saveDialog = page.getByRole("dialog", { name: SAVE });
    await expect(saveDialog).toBeVisible();
    const dialogBox = await box(saveDialog);
    expect.soft(dialogBox.width, "nested dialog width at the Cloud-iframe 320px viewport").toBeLessThanOrEqual(289);

    const actions: Array<[string, Rect]> = [
      ["cancel", await box(saveDialog.getByRole("button", { name: CANCEL }))],
      ["save", await box(saveDialog.getByRole("button", { name: SAVE }))],
    ];
    for (const [name, rect] of actions) {
      expect.soft(inside(rect, dialogBox), `save ${name} box ${JSON.stringify(rect)} escapes dialog ${JSON.stringify(dialogBox)}`).toBe(true);
      expect.soft(rect.height, `save ${name} touch height`).toBeGreaterThanOrEqual(44);
    }
    await captureLayoutThemes(page, process.env.PIP103_APP_PROOF_DIR, "connections-filter-save-320-ru");

    // Keyboard cancel: activating the nested dialog's Cancel closes only that
    // dialog — Escape is owned by the workspace's mobile-overlay handler — and
    // leaves the unsaved draft intact, so Apply stays available.
    await saveDialog.getByRole("button", { name: CANCEL }).focus();
    await page.keyboard.press("Enter");
    await expect(saveDialog).toBeHidden();
    await expect(dialog).toBeVisible();
    await expect(apply).toBeEnabled();
  });
});
