/**
 * Hierarchy-scope registry parity guard (see the extension guide in
 * `../hierarchyScope.ts`). A new hierarchy level (a new id-valued attribute
 * key) is added by registering it in that one module + both locales; this test
 * turns every skipped step into a red test instead of a silently broken tab:
 * missing entity source → no checkbox list; missing locale label → a raw
 * `rack_id` token as the tab/chip label.
 */
import { describe, expect, it } from "vitest";

import en from "@/shared/i18n/locales/en";
import ru from "@/shared/i18n/locales/ru";
import {
  HIERARCHY_KEYS,
  HIERARCHY_SCOPE_DEFS,
  ID_CONSTRAINT_KEYS,
  ID_KEY_ENTITY,
} from "../hierarchyScope";

// RU and EN differ structurally (plural-form keys), so the accessor takes the
// locale as unknown and narrows to just the dictionary under test.
const constraintKeyLabels = (locale: unknown): Record<string, string> =>
  (locale as {
    accessControl: { policies: { constraintKeyLabels: Record<string, string> } };
  }).accessControl.policies.constraintKeyLabels;

describe("hierarchy scope registry", () => {
  it("every hierarchy tab has an inventory entity source", () => {
    for (const key of HIERARCHY_KEYS) {
      expect(ID_KEY_ENTITY[key], `hierarchy key "${key}" has no ID_KEY_ENTITY source`).toBeTruthy();
    }
  });

  it("every id-valued key is exposed as a hierarchy tab (no orphan sources)", () => {
    // An entity source without a tab would resolve names but be un-authorable
    // from the scope section — most likely a half-finished addition.
    for (const key of ID_CONSTRAINT_KEYS) {
      expect(
        HIERARCHY_KEYS.includes(key),
        `id key "${key}" has an entity source but no hierarchy tab`,
      ).toBe(true);
    }
  });

  it("tab keys are unique", () => {
    expect(new Set(HIERARCHY_KEYS).size).toBe(HIERARCHY_SCOPE_DEFS.length);
  });

  it.each([
    ["ru", ru],
    ["en", en],
  ] as const)("every hierarchy key has a %s constraintKeyLabels label", (_name, locale) => {
    const labels = constraintKeyLabels(locale);
    for (const key of HIERARCHY_KEYS) {
      expect(
        typeof labels[key] === "string" && labels[key].length > 0,
        `hierarchy key "${key}" has no ${_name} label under accessControl.policies.constraintKeyLabels`,
      ).toBe(true);
    }
  });
});
