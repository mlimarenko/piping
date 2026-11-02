import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Anti-recurrence guard for the searchable-dropdown-in-a-modal-dialog bug.
 *
 * Root cause: a portaled dropdown
 * panel (Radix `PopoverContent`, always portaled to `document.body`) lives
 * outside a modal Radix `Dialog`'s `react-remove-scroll` lock tree. The lock's
 * non-passive document-level wheel/touchmove handler `preventDefault`s any
 * scroll whose target is outside that tree — so the panel's scrollbar paints
 * but the wheel never scrolls it. The fix was to render picker panels INLINE
 * (no portal) inside the dialog's own DOM subtree.
 *
 * This test fails loudly if a picker shell ever reintroduces a portaled
 * panel, or if the shared `CommandList` loses the CSS invariants (native
 * wheel-scrollable surface) the fix depends on.
 */

const here = dirname(fileURLToPath(import.meta.url));
const uiShared = resolve(here, ".");
const uiPrimitives = resolve(here, "../ui-primitives");

const PICKER_SHELL_FILES = [
  "CommandPicker.tsx",
  "SearchableSelect.tsx",
  "MultiSelect.tsx",
];

const PORTAL_IMPORT_PATTERN = /PopoverContent|PopoverPrimitive/;

describe("picker scroll guard (anti-recurrence)", () => {
  it.each(PICKER_SHELL_FILES)(
    "%s does not import a portaled Popover panel",
    (fileName) => {
      const source = readFileSync(resolve(uiShared, fileName), "utf-8");
      expect(
        PORTAL_IMPORT_PATTERN.test(source),
        `${fileName} imports PopoverContent/PopoverPrimitive — a portaled panel ` +
          "escapes a modal Dialog's react-remove-scroll lock tree and its wheel " +
          "events get swallowed (scrollbar paints, wheel is dead). Picker panels " +
          "must render inline via CommandPickerInline, not portaled via Popover. " +
          "See CommandPicker.tsx's module doc comment for the full root cause.",
      ).toBe(false);
    },
  );

  it("CommandList keeps the native wheel-scrollable surface classes", () => {
    const source = readFileSync(resolve(uiPrimitives, "command.tsx"), "utf-8");
    const listMatch = /CommandPrimitive\.List[\s\S]*?className=\{cn\(\s*"([^"]+)"/.exec(source);
    expect(
      listMatch,
      "Could not locate CommandPrimitive.List's base className in command.tsx; " +
        "update this guard if the component was restructured.",
    ).not.toBeNull();

    const baseClasses = listMatch![1]!.split(/\s+/);
    expect(baseClasses).toEqual(
      expect.arrayContaining(["overflow-y-auto", "overscroll-contain"]),
    );
    expect(baseClasses.some((cls) => /^max-h-/.test(cls))).toBe(true);
  });
});
