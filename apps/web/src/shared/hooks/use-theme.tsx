import { useEffect, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useSetting } from "@/shared/settings";
import {
  readCachedSettingValue,
  readRetiredLocalValue,
  retireLocalKey,
  writeCachedSettingValue,
} from "@/shared/settings";
import { normalizeThemeSetting } from "@/shared/settings";

import { ThemeContext, type Theme } from "./theme-context";

const LS_KEY = "piping-theme";
const THEME_SETTING_KEY = "app.theme";

interface ThemePreference {
  mode: Theme;
}

const DEFAULT_THEME: Theme = "system";

function getSystemTheme(): "light" | "dark" {
  if (typeof window === "undefined") return "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/** Read the cached theme from localStorage for instant first-paint. */
function readCachedTheme(): Theme {
  const cached = readCachedSettingValue(THEME_SETTING_KEY, normalizeThemeSetting);
  if (cached) return cached.mode;
  const retired = readRetiredLocalValue(LS_KEY, normalizeThemeSetting);
  if (retired) return retired.mode;
  return DEFAULT_THEME;
}

/** Write theme to localStorage as a fast-load cache (not the source of truth). */
function writeCachedTheme(t: Theme): void {
  writeCachedSettingValue(THEME_SETTING_KEY, { mode: t });
  retireLocalKey(LS_KEY);
}

/** Apply the effective theme to the document root. */
function applyThemeToDom(effective: "light" | "dark"): void {
  const root = document.documentElement;
  root.classList.remove("light", "dark");
  root.classList.add(effective);
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  // Fast-path: read from localStorage for instant first-paint (no flicker).
  const [localTheme, setLocalTheme] = useState<Theme>(readCachedTheme);
  const [systemTheme, setSystemTheme] = useState(getSystemTheme);

  // Detect authentication by checking the auth/me query cache.
  const authQuery = useQuery<unknown>({
    queryKey: ["auth", "me"],
    queryFn: () => undefined,
    enabled: false,
  });
  const isAuthenticated = authQuery.data !== undefined;

  const {
    value: themePref,
    setValue: setThemePref,
    isLoading,
  } = useSetting<ThemePreference>(
    THEME_SETTING_KEY,
    { mode: DEFAULT_THEME },
    {
      enabled: isAuthenticated,
      normalize: normalizeThemeSetting,
      bootstrapCache: true,
    },
  );

  // Active theme: once loaded from API use it; before hydration use localStorage.
  const activeTheme = (!isLoading && isAuthenticated) ? themePref.mode : localTheme;
  const resolvedTheme = activeTheme === "system" ? systemTheme : activeTheme;

  // Apply theme class to DOM and write localStorage cache on every change.
  useEffect(() => {
    applyThemeToDom(resolvedTheme);
    writeCachedTheme(activeTheme);
  }, [activeTheme, resolvedTheme]);

  // Keep the effective theme observable by controls as well as by CSS.
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => {
      setSystemTheme(getSystemTheme());
    };
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

  const setTheme = (t: Theme) => {
    setLocalTheme(t);
    writeCachedTheme(t);
    if (isAuthenticated) {
      setThemePref({ mode: t });
    }
  };

  return (
    <ThemeContext.Provider value={{ theme: activeTheme, resolvedTheme, setTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}
