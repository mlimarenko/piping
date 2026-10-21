/**
 * Demo-only language handshake.
 *
 * The public portal embeds this demo under `/demo-app/` and names the language
 * it wants in an explicit `?lang=` query. That query is the single source of
 * truth for the embedded demo language: the portal and the demo share no
 * storage, so there is no `i18nextLng` (or any other key) contract between them.
 *
 * The value is untrusted input from the embedding page, so it is validated at
 * this boundary and folded to a ship-supported base code the same way the app's
 * own `load: "languageOnly"` resolution folds regional tags (`ru-RU` → `ru`).
 */
import type { SupportedLanguage } from "@/shared/i18n";

/**
 * Languages the demo can render. Mirror of `SUPPORTED_LANGUAGES` kept as a local
 * value so importing this boundary does not pull the i18n runtime (and its eager
 * init side effect) into every caller; the type keeps the two in lockstep.
 */
const DEMO_LANGUAGES: readonly SupportedLanguage[] = ["en", "ru"];

/**
 * Normalise an explicit demo `lang` value to a shipped language. Accepts a bare
 * code (`ru`) or a regional tag (`ru-RU`, `en_US`) and folds it to its base.
 * Returns `undefined` for anything the demo cannot render, so the caller keeps
 * the native detector/default instead of guessing.
 */
export function normalizeDemoLanguage(
  value: string | null | undefined,
): SupportedLanguage | undefined {
  if (!value) return undefined;
  const [base] = value.trim().toLowerCase().split(/[-_]/);
  return DEMO_LANGUAGES.find((language) => language === base);
}

/** The demo language named by a `?lang=` query string, if it is one we ship. */
export function demoLanguageFromSearch(search: string): SupportedLanguage | undefined {
  return normalizeDemoLanguage(new URLSearchParams(search).get("lang"));
}

/** The demo language named by the current page URL, if present and supported. */
export function demoLanguageFromLocation(): SupportedLanguage | undefined {
  return demoLanguageFromSearch(window.location.search);
}
