import { useState } from "react";
import { readBrowserLocalSetting, writeBrowserLocalSetting } from "@/shared/settings/local-cache";

const PREFERENCE_KEY = "connections.inspector.expandedSections";
const EMPTY: string[] = [];

function normalizeSections(value: unknown): string[] {
  return Array.isArray(value)
    ? [...new Set(value.filter((key): key is string => typeof key === "string" && key.length > 0))]
    : EMPTY;
}

/** Section preferences belong to this browser, not a host or a server setting. */
export function useInspectorSections() {
  const [expandedSections, setExpandedSections] = useState(() =>
    readBrowserLocalSetting(PREFERENCE_KEY, EMPTY, normalizeSections),
  );
  const toggleSection = (key: string) => {
    const next = expandedSections.includes(key)
      ? expandedSections.filter(section => section !== key)
      : [...expandedSections, key];
    writeBrowserLocalSetting(PREFERENCE_KEY, next, normalizeSections);
    setExpandedSections(next);
  };
  return { expandedSections, toggleSection };
}
