import { useCallback, useEffect, useMemo } from "react";
import { useSetting } from "@/shared/settings";
import {
  readCachedSettingValue,
  readRetiredLocalValue,
  retireLocalKey,
  writeCachedSettingValue,
} from "@/shared/settings";
import {
  applyUiScale,
  DEFAULT_UI_SCALE,
  sanitizeUiScale,
  UI_SCALE_PREFERENCE_KEY,
} from "@/shared/ui-scale";

const UI_SCALE_STORAGE_KEY = "piping.uiScale";

function readStoredUiScale(): number {
  return (
    readCachedSettingValue(UI_SCALE_PREFERENCE_KEY, sanitizeUiScale) ??
    readRetiredLocalValue(UI_SCALE_STORAGE_KEY, sanitizeUiScale) ??
    DEFAULT_UI_SCALE
  );
}

function writeStoredUiScale(scale: number): void {
  writeCachedSettingValue(UI_SCALE_PREFERENCE_KEY, sanitizeUiScale(scale));
  retireLocalKey(UI_SCALE_STORAGE_KEY);
}

export function useUiScalePreference(enabled = true) {
  const defaultScale = useMemo(() => readStoredUiScale(), []);
  const preference = useSetting<number>(UI_SCALE_PREFERENCE_KEY, defaultScale, {
    debounceMs: 200,
    enabled,
    normalize: sanitizeUiScale,
    bootstrapCache: true,
  });
  const value = sanitizeUiScale(preference.value);

  const setValue = useCallback(
    (next: number | ((prev: number) => number)) => {
      preference.setValue((previous) => {
        const sanitizedPrevious = sanitizeUiScale(previous);
        const resolved = typeof next === "function" ? next(sanitizedPrevious) : next;
        const sanitizedNext = sanitizeUiScale(resolved);
        writeStoredUiScale(sanitizedNext);
        return sanitizedNext;
      });
    },
    [preference],
  );

  useEffect(() => {
    writeStoredUiScale(value);
  }, [value]);

  return {
    ...preference,
    value,
    setValue,
  };
}

export function useApplyUiScale(scale: number): void {
  useEffect(() => {
    applyUiScale(scale);
  }, [scale]);
}
