/**
 * Locale parity guard.
 *
 * Every *logical* key that exists in EN must exist in RU and vice-versa.
 * CLDR plural suffixes (_zero, _one, _two, _few, _many, _other) are
 * normalised away before comparison: RU correctly uses _few/_many where
 * EN only needs _one/_other — that is expected and must not be flagged.
 */
import { describe, expect, it } from "vitest";

import en from "../locales/en";
import ru from "../locales/ru";

const PLURAL_SUFFIX = /_(zero|one|two|few|many|other)$/;

/**
 * Recursively collect all leaf key paths from a nested object, then strip
 * CLDR plural suffixes and deduplicate so that e.g.
 *   sessionsCount_one / sessionsCount_few / sessionsCount_other
 * all collapse to the single logical key `sessionsCount`.
 */
function collectKeys(obj: Record<string, unknown>, prefix = ""): Set<string> {
  const result = new Set<string>();
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value !== null && typeof value === "object" && !Array.isArray(value)) {
      for (const child of collectKeys(value as Record<string, unknown>, path)) {
        result.add(child);
      }
    } else {
      // Normalise: strip plural suffix, then store.
      result.add(path.replace(PLURAL_SUFFIX, ""));
    }
  }
  return result;
}

describe("EN ↔ RU locale parity", () => {
  const enKeys = collectKeys(en);
  const ruKeys = collectKeys(ru);

  it("every key in EN exists in RU", () => {
    const onlyInEn = [...enKeys].filter((k) => !ruKeys.has(k));
    expect(
      onlyInEn,
      `Keys present in EN but missing in RU:\n  ${onlyInEn.join("\n  ")}`,
    ).toEqual([]);
  });

  it("every key in RU exists in EN", () => {
    const onlyInRu = [...ruKeys].filter((k) => !enKeys.has(k));
    expect(
      onlyInRu,
      `Keys present in RU but missing in EN:\n  ${onlyInRu.join("\n  ")}`,
    ).toEqual([]);
  });
});
