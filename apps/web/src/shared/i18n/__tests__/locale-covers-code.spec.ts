/**
 * Every key the UI asks for must exist in BOTH locales.
 *
 * The parity guard next door compares en.ts against ru.ts. That catches a key
 * added to one file and forgotten in the other — but it is blind to the case
 * that actually ships English text to Russian users: `t("some.key", "English
 * fallback")` where the key is in NEITHER file. Parity holds (both lack it),
 * the test passes, and i18next quietly renders the inline fallback.
 *
 * That is not hypothetical. The CCTV recording timeline shipped with nine such
 * keys, so a Russian operator read "Prev day" / "Next day" / "No recorded
 * footage this day" in the middle of a Russian console, and no gate objected.
 *
 * A fallback is a development convenience, not a translation. This test treats
 * it as one.
 */
import { globSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, relative } from "node:path";

import { describe, expect, it } from "vitest";

import en from "../locales/en";
import ru from "../locales/ru";

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = join(here, "..", "..", "..");

/** `t("a.b.c"` / `t('a.b.c'` — the literal-key call form. */
const T_CALL = /\bt\(\s*["']([a-z]\w*(?:\.\w+)+)["']/g;

/** CLDR plural suffixes, mirrored from the parity guard. */
const PLURAL_SUFFIX = /_(zero|one|two|few|many|other)$/;

/**
 * Code-unit order, so the failure message reads the same on every machine.
 * `localeCompare` would not: its order depends on the host locale, and this
 * text is diffed between CI runs.
 */
function byCodeUnit(left: string, right: string): number {
  if (left < right) return -1;
  return left > right ? 1 : 0;
}

function leafKeys(value: unknown, prefix = "", out = new Set<string>()): Set<string> {
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      leafKeys(child, prefix ? `${prefix}.${key}` : key, out);
    }
  } else if (prefix) {
    out.add(prefix.replace(PLURAL_SUFFIX, ""));
  }
  return out;
}

/**
 * Keys the code requests, by source file.
 *
 * Only the literal-string call form is matched. A key built at runtime
 * (`t(\`nav.${section}\`)`) cannot be resolved statically, so it is out of
 * scope here rather than guessed at — this gate reports what it can prove.
 */
function requestedKeys(): Map<string, Set<string>> {
  const byFile = new Map<string, Set<string>>();
  const files = globSync("**/*.{ts,tsx}", { cwd: srcRoot })
    .filter((file) => !/\.(test|spec)\.tsx?$/.test(file))
    .filter((file) => !file.includes("shared/i18n/locales/"));
  for (const file of files) {
    const source = readFileSync(join(srcRoot, file), "utf8");
    for (const match of source.matchAll(T_CALL)) {
      // The group is always present when the pattern matches, but
      // `noUncheckedIndexedAccess` types it as optional and is right to: a
      // future edit to T_CALL could drop the group.
      const key = match[1];
      if (!key) continue;
      const bucket = byFile.get(key) ?? new Set<string>();
      bucket.add(file);
      byFile.set(key, bucket);
    }
  }
  return byFile;
}

describe("locales cover the keys the code asks for", () => {
  const enKeys = leafKeys(en);
  const ruKeys = leafKeys(ru);
  const requested = requestedKeys();

  it("finds t() calls to check, so a broken scan cannot pass silently", () => {
    expect(requested.size).toBeGreaterThan(200);
  });

  it("has no key that resolves to its inline English fallback", () => {
    const missing: string[] = [];
    for (const key of [...requested.keys()].sort(byCodeUnit)) {
      const absentIn = [!enKeys.has(key) && "en", !ruKeys.has(key) && "ru"].filter(
        Boolean,
      ) as string[];
      if (absentIn.length === 0) continue;
      const files = requested.get(key) ?? new Set<string>();
      const where = [...files]
        .map((file) => relative(".", file))
        .sort(byCodeUnit)
        .slice(0, 2)
        .join(", ");
      missing.push(`${key} — missing from ${absentIn.join(" and ")} (used in ${where})`);
    }

    expect(
      missing,
      `These keys fall back to the English string written at the call site, so a\n` +
        `non-English UI silently renders English. Add them to both locale files:\n` +
        missing.join("\n"),
    ).toEqual([]);
  });
});
