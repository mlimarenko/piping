import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import LanguageDetector from "i18next-browser-languagedetector";
import {
  fetchEffectiveSetting,
  normalizeLanguageSetting,
  readCachedSettingValue,
  readRetiredLocalValue,
  retireLocalKey,
  upsertUserSetting,
  writeCachedSettingValue,
} from "@/shared/settings";

const PREF_KEY = "app.language";
const LS_KEY = "i18nextLng";

export const SUPPORTED_LANGUAGES = ["en", "ru"] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

/**
 * Dictionaries are fetched on demand and never bundled into the entry chunk.
 * Together they weigh ~144 KB gzipped, and a session renders exactly one of
 * them; each `import()` below becomes its own chunk so a cold load pays for
 * one dictionary instead of two.
 */
const dictionaryLoaders: Record<SupportedLanguage, () => Promise<{ default: object }>> = {
  en: () => import("./locales/en"),
  ru: () => import("./locales/ru"),
};

const bundleLoads = new Map<SupportedLanguage, Promise<void>>();

function isSupported(value: string | undefined): value is SupportedLanguage {
  return value === "en" || value === "ru";
}

/** Fetch and register one dictionary. Idempotent, safe to call concurrently. */
export function ensureLanguageBundle(language: SupportedLanguage): Promise<void> {
  const pending = bundleLoads.get(language);
  if (pending) return pending;

  const load = dictionaryLoaders[language]()
    .then((module) => {
      i18n.addResourceBundle(language, "translation", module.default, true, true);
    })
    .catch((error: unknown) => {
      // Drop the rejected promise so a later attempt can retry instead of
      // permanently caching the failure.
      bundleLoads.delete(language);
      throw error;
    });

  bundleLoads.set(language, load);
  return load;
}

/**
 * The language i18next settled on, narrowed to one we actually ship.
 * `load: "languageOnly"` already folds regional codes (`ru-RU`) into their base
 * (`ru`); this only guards against a navigator locale we do not translate.
 */
function activeLanguage(): SupportedLanguage {
  const candidates = [i18n.resolvedLanguage, ...(i18n.languages ?? [])];
  return candidates.find(isSupported) ?? "en";
}

const cachedLanguage = readCachedSettingValue(PREF_KEY, normalizeLanguageSetting)?.lng;

const initPromise = i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    // Empty on purpose — the active dictionary is registered below, before
    // anything renders. Rendering before it lands would flash raw keys.
    resources: {},
    fallbackLng: "en",
    lng: cachedLanguage,
    supportedLngs: [...SUPPORTED_LANGUAGES],
    load: "languageOnly",
    interpolation: { escapeValue: false },
    detection: {
      order: ["navigator"],
      caches: [],
    },
  })
  .then(async () => {
    await ensureLanguageBundle(activeLanguage());
    document.documentElement.lang = activeLanguage();
  });

/**
 * Resolves once the active dictionary is registered. Every entry point must
 * await this before mounting React.
 */
export function initI18n(): Promise<void> {
  return initPromise;
}

/**
 * Switch languages at runtime, loading the target dictionary first. Callers
 * must use this rather than `i18n.changeLanguage`, which would activate a
 * language that has no resources behind it.
 */
export async function changeAppLanguage(language: SupportedLanguage): Promise<void> {
  await ensureLanguageBundle(language);
  if (i18n.resolvedLanguage !== language) {
    await i18n.changeLanguage(language);
  }
  document.documentElement.lang = language;
}

/**
 * Warm the dictionary we did NOT boot with, off the critical path. Two reasons:
 * `fallbackLng: "en"` can only render English for a key missing from the active
 * dictionary while English is loaded, and a later language switch becomes
 * instant. Call after the first render.
 */
export function preloadFallbackLanguage(): void {
  const active = activeLanguage();
  for (const language of SUPPORTED_LANGUAGES) {
    if (language !== active) {
      void ensureLanguageBundle(language).catch(() => {
        // A warm-up failure is not user visible; the active language still works.
      });
    }
  }
}

/**
 * Load the user's language preference from the API and apply it.
 * Call this once after successful authentication (e.g. from AppShell).
 * Falls back gracefully to the localStorage/browser-detected language
 * if the API is unreachable or returns no preference.
 */
export async function loadLanguagePreference(): Promise<void> {
  try {
    const setting = await fetchEffectiveSetting(PREF_KEY);
    const pref = normalizeLanguageSetting(setting.value);
    if (setting.source !== "unset") {
      writeCachedSettingValue(PREF_KEY, pref);
      retireLocalKey(LS_KEY);
      await changeAppLanguage(pref.lng);
      return;
    }
    await migrateLanguageIfNeeded();
  } catch {
    // Keep current language (browser-detected / cached) if the API is unreachable.
  }
}

async function migrateLanguageIfNeeded(): Promise<void> {
  try {
    const cached = readRetiredLocalValue(LS_KEY, normalizeLanguageSetting);
    if (!cached) return;
    await upsertUserSetting(PREF_KEY, cached);
    writeCachedSettingValue(PREF_KEY, cached);
    retireLocalKey(LS_KEY);
  } catch {
    /* ignore migration errors */
  }
}


export default i18n;
